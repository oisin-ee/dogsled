//! Exercise the real commands with private state and fake Herdr/SSH on PATH.
#![allow(clippy::unwrap_used, clippy::panic)]

#[path = "cli/claim.rs"]
mod claim;
#[path = "cli/close.rs"]
mod close;
#[path = "cli/context.rs"]
mod context;
#[path = "cli/done.rs"]
mod done;
#[path = "cli/drop.rs"]
mod drop;
#[path = "cli/git_env.rs"]
mod git_env;
#[path = "cli/host_store.rs"]
mod host_store;
#[path = "cli/inbox.rs"]
mod inbox;
#[path = "cli/jobs.rs"]
mod jobs;
#[path = "cli/launch_retry.rs"]
mod launch_retry;
#[path = "cli/lead.rs"]
mod lead;
#[path = "cli/legacy_continue.rs"]
mod legacy_continue;
#[path = "cli/lifecycle.rs"]
mod lifecycle;
#[path = "cli/machines.rs"]
mod machines;
#[path = "cli/mcp.rs"]
mod mcp;
#[path = "cli/observation.rs"]
mod observation;
#[path = "cli/place.rs"]
mod place;
#[path = "cli/placement.rs"]
mod placement;
#[path = "cli/preparation_retirement.rs"]
mod preparation_retirement;
#[path = "cli/preservation.rs"]
mod preservation;
#[path = "cli/profiles.rs"]
mod profiles;
#[path = "cli/remote_diagnostics.rs"]
mod remote_diagnostics;
#[path = "cli/request_validity.rs"]
mod request_validity;
#[path = "cli/retention.rs"]
mod retention;
#[path = "cli/review.rs"]
mod review;
#[path = "cli/review_fixes.rs"]
mod review_fixes;
#[path = "cli/service_lanes.rs"]
mod service_lanes;
#[path = "cli/signals.rs"]
mod signals;
#[path = "cli/stop_sweep.rs"]
mod stop_sweep;
#[path = "cli/sweep.rs"]
mod sweep;
#[path = "cli/sweep_ownership.rs"]
mod sweep_ownership;
#[path = "cli/transcripts.rs"]
mod transcripts;
#[path = "cli/unit_requests.rs"]
mod unit_requests;
#[path = "cli/unit_seeded.rs"]
mod unit_seeded;
#[path = "cli/watchdog.rs"]
mod watchdog;
#[path = "cli/web_state.rs"]
mod web_state;

use serde_json::{Value, json};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// The bound every real-process wait in these tests uses. A process that never ends fails
/// its test on this bound instead of stalling the suite.
/// This bound catches hangs and must exceed full-suite contention.
pub(crate) fn outcome_identity(mut value: Value) -> Value {
    value.as_object_mut().unwrap().remove("finished_ms");
    value
}

const PROCESS_DEADLINE: Duration = Duration::from_secs(120);

/// The state the guard and its fallback share. A finished process has released its pid,
/// and the host may then hand that pid to another process. A signal sent after that can
/// kill a process that is not this test's. The guard and every fallback take this lock,
/// so the signal and the reap keep one order: the signal comes first, and no signal
/// follows the reap.
#[derive(Debug, Default)]
struct OwnedState {
    /// Whether the guard is finished with the owned process: it has been killed and
    /// reaped, or it was already gone. A finished process has left its group, so the
    /// host may already have handed its group id to another group. The guard and every
    /// fallback signal nothing once this is set.
    finished: bool,
}

/// A process a test owns. The guard kills the process and its group and waits for it to
/// end, so a failed assertion never strands a wrapper or a command it started. A process
/// this test spawned is the test's own child, so the test reaps it and proves its exit.
#[derive(Debug)]
pub(crate) struct Owned {
    /// The process to kill.
    pid: u32,
    /// The process group to kill, when the process leads a group of its own.
    group: Option<u32>,
    /// The child to reap, when this test spawned it.
    child: Option<std::process::Child>,
    /// Whether the guard already killed and reaped this process. A second call is a
    /// no-op, so a later field drop signals nothing.
    terminated: bool,
    /// The state the guard and every fallback share.
    state: Arc<Mutex<OwnedState>>,
}

/// A handle that kills an owned process group through the guard's own state. It signals
/// only while the child is unreaped, so a group id the host may have reused is never
/// signalled.
#[derive(Debug, Clone)]
pub(crate) struct GroupKiller {
    group: Option<u32>,
    state: Arc<Mutex<OwnedState>>,
}

impl GroupKiller {
    /// Kill the group, unless the guard is already finished with the process.
    pub(crate) fn kill(&self) {
        // The lock is taken without waiting. A fallback that waits on the guard would
        // hang the test it tries to release. The fallback signals the group only when
        // it holds the lock and the guard is not finished, because a finished guard has
        // already killed the group before it reaped the child, so skipping is right.
        let Ok(state) = self.state.try_lock() else {
            return;
        };
        if state.finished {
            return;
        }
        if let Some(group) = self.group
            && let Ok(group) = i32::try_from(group)
        {
            let _ = nix::sys::signal::killpg(
                nix::unistd::Pid::from_raw(group),
                nix::sys::signal::SIGKILL,
            );
        }
    }
}

impl Owned {
    /// A handle that kills this guard's group while the child is unreaped.
    pub(crate) fn group_killer(&self) -> GroupKiller {
        GroupKiller {
            group: self.group,
            state: Arc::clone(&self.state),
        }
    }

    /// The shared state, so a signal and a reap keep one order.
    ///
    /// The state is cloned before it is locked, so the lock does not borrow this guard
    /// and the guard can still reach its child while the lock is held.
    fn shared_state(&self) -> Arc<Mutex<OwnedState>> {
        Arc::clone(&self.state)
    }

    /// Start `command` in its own process group, so the guard kills its descendants too.
    /// The caller's own stdio settings are kept. The child is this test's own, so the
    /// test reaps it and reads its exit status.
    pub(crate) fn spawn(command: &mut Command) -> Self {
        use std::os::unix::process::CommandExt as _;
        command.process_group(0);
        let child = command.spawn().unwrap();
        let pid = child.id();
        Self {
            pid,
            group: Some(pid),
            child: Some(child),
            terminated: false,
            state: Arc::default(),
        }
    }

    /// The child this guard owns, so a test can take its streams.
    pub(crate) fn child_mut(&mut self) -> &mut std::process::Child {
        let child = self.child.as_mut();
        assert!(child.is_some(), "this process was not spawned by the test");
        child.unwrap()
    }

    /// Wait for the child to exit, or fail the test on the bound. A child that never exits
    /// fails its test instead of hanging it. The status comes from `try_wait`, so the
    /// child is reaped: the exit is proved, not inferred.
    pub(crate) fn wait(&mut self) -> std::process::ExitStatus {
        let deadline = Instant::now() + PROCESS_DEADLINE;
        loop {
            // The lock is held across the reap, so a fallback never signals a group the
            // host has already given to another group.
            let state = Arc::clone(&self.state);
            let mut guard = state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let child = self.child_mut();
            match child.try_wait() {
                Ok(Some(status)) => {
                    // The child is reaped, so its group id may already belong to another
                    // group. The guard and every fallback signal nothing from now on.
                    guard.finished = true;
                    return status;
                }
                Ok(None) => {}
                Err(error) => panic!("the child could not be waited for: {error}"),
            }
            drop(guard);
            assert!(
                Instant::now() < deadline,
                "the child did not exit within {PROCESS_DEADLINE:?}"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Adopt a process this test did not spawn, such as a command a wrapper started. The
    /// process is not the test's child, so the test cannot reap it and the host's init
    /// does that. The guard kills the process and its group and waits for it to end. The
    /// group is only killed when it is not the test's own group, so the guard never kills
    /// the test.
    pub(crate) fn adopt(pid: u32) -> Self {
        let group = process_group(pid).filter(|group| *group != own_group());
        Self {
            pid,
            group,
            child: None,
            terminated: false,
            state: Arc::default(),
        }
    }
}

impl Drop for Owned {
    fn drop(&mut self) {
        self.terminate();
    }
}

impl Owned {
    /// Kill the process and its group and wait for it to end. The call is idempotent: a
    /// second call does nothing, so a later field drop signals nothing.
    fn terminate(&mut self) {
        if self.terminated {
            return;
        }
        self.terminated = true;
        // Every signal and the reap happen under this lock, so a `GroupKiller` never
        // signals after the reap.
        let shared = self.shared_state();
        let mut state = shared
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.finished {
            return;
        }
        // A child this guard spawned is the test's own child, so the test reaps it. A
        // child that already ended has been reaped too, so its group id may already
        // belong to another group, and the guard signals nothing then.
        let child_gone = match self.child.as_mut() {
            Some(child) => matches!(child.try_wait(), Ok(Some(_))),
            None => false,
        };
        if child_gone {
            state.finished = true;
            return;
        }
        // The group holds the processes that share the leader's group. A command that
        // leads a group of its own is not in it, so the test adopts that command
        // separately. The leader is not reaped yet, so the group id is still this
        // test's own.
        if let Some(group) = self.group
            && let Ok(group) = i32::try_from(group)
        {
            let _ = nix::sys::signal::killpg(
                nix::unistd::Pid::from_raw(group),
                nix::sys::signal::SIGKILL,
            );
        }
        if let Some(child) = self.child.as_mut() {
            // The child is reaped here, under the lock. Its exit status is the proof that
            // it ended, so the guard never signals a process id the host has handed to
            // someone else.
            let _ = child.kill();
            let _ = child.wait();
            state.finished = true;
            return;
        }
        // A process this guard did not spawn is not the test's child, so the test cannot
        // reap it. The host's init reaps it, and may reuse the pid, so the guard
        // signals the process once and never again.
        let Ok(pid) = i32::try_from(self.pid) else {
            state.finished = true;
            return;
        };
        let _ = nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), nix::sys::signal::SIGKILL);
        state.finished = true;
        drop(state);
        // The guard waits for the process to end, so no process outlives its test.
        let deadline = Instant::now() + PROCESS_DEADLINE;
        while !process_gone(self.pid) {
            assert!(
                Instant::now() < deadline,
                "process {} did not leave within {PROCESS_DEADLINE:?}",
                self.pid
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

/// A command a test started in the background. The test owns the command as its own
/// child, so the test reaps it and reads its exit status. A command that never ends
/// fails the test on the bound instead of hanging it. Dropping the guard kills the
/// command and its group and reaps it, so the command does not outlive the test.
pub(crate) struct Background {
    /// The command the test owns. The guard kills the command and its group and reaps it
    /// before the guard joins the reader.
    owned: Owned,
    /// The thread that drains the command's output, so a full pipe never blocks it.
    reader: Option<std::thread::JoinHandle<()>>,
    /// The command's output, sent when the command's streams close.
    output: std::sync::mpsc::Receiver<(String, String)>,
}

impl Background {
    /// The command's output, or fail the test on the bound. A command that never ends
    /// fails the test instead of hanging it.
    pub(crate) fn output(&self) -> (String, String) {
        self.output
            .recv_timeout(PROCESS_DEADLINE)
            .unwrap_or_else(|error| panic!("the command wrote no output: {error}"))
    }

    /// Wait for the command to end, or fail the test on the bound. The status comes from
    /// `try_wait`, so the child is reaped: the exit is proved, not inferred.
    pub(crate) fn wait(&mut self) -> std::process::ExitStatus {
        self.owned.wait()
    }
}

impl Drop for Background {
    fn drop(&mut self) {
        // The owned command group is killed and reaped first. The reader ends only when
        // the command's streams close, so joining the reader before the kill waits on a
        // command that never ends.
        self.owned.terminate();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

/// A FIFO a test owns. The guard removes it, so a failed assertion leaves no rendezvous
/// behind that a later test could block on.
#[derive(Debug)]
pub(crate) struct Fifo {
    path: PathBuf,
    /// Keep releases buffered even before the peer opens its reader.
    _hold: fs::File,
}

impl Fifo {
    /// A FIFO at `path` in the rig's directory.
    pub(crate) fn new(dir: &Path, name: &str) -> Self {
        let path = dir.join(name);
        assert!(
            Command::new("mkfifo")
                .arg(&path)
                .status()
                .unwrap()
                .success()
        );
        let hold = open_fifo(&path);
        Self { path, _hold: hold }
    }

    /// The path of this FIFO.
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// Open this FIFO for reading and writing, and keep it open. A writer on the other
    /// side never waits for an open that may never come, and the FIFO stays open through
    /// the acknowledgement.
    pub(crate) fn hold(&self) -> fs::File {
        open_fifo(&self.path)
    }
}

impl Drop for Fifo {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// Open a FIFO without blocking. The FIFO is opened for reading and writing, so neither
/// side waits for an open that may never come.
pub(crate) fn open_fifo(path: &Path) -> fs::File {
    use std::os::unix::fs::OpenOptionsExt as _;
    fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(nix::libc::O_NONBLOCK)
        .open(path)
        .unwrap()
}

/// Read a FIFO's content within `bound`, or `None` when the bound passes. A writer that
/// never arrives must not hang the test. An empty read means no writer has opened the FIFO
/// yet, so the read keeps waiting for the writer rather than ending the wait early.
pub(crate) fn read_fifo_within(path: &Path, what: &str, bound: Duration) -> Option<String> {
    let mut file = open_fifo(path);
    let deadline = Instant::now() + bound;
    let mut text = String::new();
    loop {
        let mut buffer = [0_u8; 256];
        match std::io::Read::read(&mut file, &mut buffer) {
            Ok(0) if !text.is_empty() => break,
            Ok(0) => {}
            Ok(n) => {
                text.push_str(&String::from_utf8_lossy(&buffer[..n]));
                if text.contains('\n') {
                    break;
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(error) => panic!("{what} could not be read: {error}"),
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    Some(text)
}

/// Read a FIFO's content, or fail the test on the bound. A writer that never arrives must
/// not hang the test.
pub(crate) fn read_fifo(path: &Path, what: &str) -> String {
    read_fifo_within(path, what, PROCESS_DEADLINE)
        .unwrap_or_else(|| panic!("{what} did not arrive within {PROCESS_DEADLINE:?}"))
}

/// Wait for `pid` to end, up to `bound`. It reports whether the process ended. A process
/// that never ends is reported on the bound, so a caller can fail on it instead of
/// waiting for it. Only a process the test did not spawn is checked this way: a process
/// the test spawned is reaped through its own child handle, and its exit status is the
/// proof that it ended.
pub(crate) fn wait_gone(pid: u32, bound: Duration) -> bool {
    let deadline = Instant::now() + bound;
    loop {
        if process_gone(pid) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// The command in `path`, or fail the test on the bound. A file that is never written
/// fails the test instead of hanging it.
pub(crate) fn read_command(path: &Path, what: &str) -> String {
    let deadline = Instant::now() + PROCESS_DEADLINE;
    loop {
        if let Ok(text) = fs::read_to_string(path)
            && !text.trim().is_empty()
        {
            return text.trim().to_owned();
        }
        assert!(
            Instant::now() < deadline,
            "{what} did not write its command within {PROCESS_DEADLINE:?}"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// The process id in `path`, or fail the test on the bound. A process that never writes it
/// fails the test instead of hanging it.
pub(crate) fn read_pid(path: &Path, what: &str) -> u32 {
    let deadline = Instant::now() + PROCESS_DEADLINE;
    loop {
        if let Ok(text) = fs::read_to_string(path)
            && let Ok(pid) = text.trim().parse::<u32>()
        {
            return pid;
        }
        assert!(
            Instant::now() < deadline,
            "{what} did not write its process id within {PROCESS_DEADLINE:?}"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Whether `pid` has ended. A process that already ended, or that waits only to be
/// reaped, has ended for the test's purpose. Only a process the test did not spawn is
/// checked this way: a process the test spawned is reaped through its own child handle,
/// and its exit status is the proof that it ended.
pub(crate) fn process_gone(pid: u32) -> bool {
    let Ok(pid) = i32::try_from(pid) else {
        return true;
    };
    if nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_err() {
        return true;
    }
    // A process that ended but waits to be reaped still holds its process id. It has
    // left, so the test does not wait for it.
    let out = Command::new("ps")
        .args(["-o", "stat=", "-p", &pid.to_string()])
        .output()
        .unwrap();
    if !out.status.success() {
        return true;
    }
    String::from_utf8_lossy(&out.stdout).trim().starts_with('Z')
}

/// The process group of `pid`, or `None` when the process already left. A process that
/// already ended has nothing left to kill, so the test does not wait for it.
pub(crate) fn process_group(pid: u32) -> Option<u32> {
    let out = Command::new("ps")
        .args(["-o", "pgid=", "-p", &pid.to_string()])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse::<u32>()
        .ok()
}

/// The process group of this test. A guard never kills this group: it holds the test.
pub(crate) fn own_group() -> u32 {
    nix::unistd::getpgrp().as_raw() as u32
}

fn isolated(program: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut cmd = Command::new(program);
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("GIT_") {
            cmd.env_remove(key);
        }
    }
    cmd
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

/// What dogsled tells each child at launch: `src/done.rs`'s `CONTRACT`, as Herdr logs it.
const CONTRACT: &str = "End every turn with exactly one of: done (whole task complete), ask (you need an answer from your owner), blocked (you cannot continue), or waiting for your own open children. Use the native tools. Commit your changes before done. Call done once during the turn that completes the whole task, then end the turn with your final reply. Dogsled closes you after that turn and keeps your branch for the owner to land. Do not call done when you need an answer, cannot continue, or own open children.";

const HERDR_SCRIPT: &str = r#"#!/bin/sh
printf '%s\n' "$*" >> "$RIG/herdr.log"
printf '%s %s\n' "${DOGSLED_TEST_ROLE:-actor}" "$*" >> "$RIG/herdr-roles.log"
# The line Herdr types for `agent start`: the kind, then each argument shell-quoted.
# A terminal refuses a line over 1024 bytes, so a longer one fails like the real thing.
if test "$1 $2" = 'agent start'; then
  (
    # A child passes its arguments after `--`; a lead passes none.
    kind=$5; shift 7
    if test "$1" = --; then shift; fi
    typed=$kind
    for arg in "$@"; do
      q=$(printf '%s' "$arg" | sed "s/'/'\\\\''/g")
      typed="$typed '$q'"
    done
    printf '%s\n' "$typed" >> "$RIG/typed.log"
    test "$(printf '%s\n' "$typed" | wc -c)" -le 1024
  ) || { echo '{"error":{"code":"agent_start_input_failed","message":"line too long"}}' >&2; exit 1; }
fi
case "$*" in
  'machine list --json') cat "$RIG/machines.json" ;;
  'agent list') cat "${RIG_AGENTS:-$RIG/agents.json}" ;;
  'pane list') cat "$RIG/panes.json" ;;
  'workspace list') printf '%s\n' '{"result":{"workspaces":[{"workspace_id":"w1","label":"caller"}]}}' ;;
  'pane process-info'*) printf '%s\n' '{"result":{"process_info":{"shell_pid":10,"foreground_processes":[{"pid":10,"name":"zsh","argv":["zsh"]}]}}}' ;;
  'worktree list --cwd'*) cat "$RIG/herdr-worktrees" ;;
  'tab create'*|'workspace create'*) printf '%s\n' '{"result":{"root_pane":{"pane_id":"w2:p1","terminal_id":"term_child"}}}' ;;
  'pane run'*)
    shift 3
    case "$*" in
      *' exec --job-validity '*) sh -c "$*" > "$RIG/job-output" 2>&1 < /dev/null & ;;
      *) sh -c "$*" > "$RIG/job-output" 2>&1 ;;
    esac
    printf '{}\n' ;;
  'pane close '*)
    mkdir -p "$RIG/at-close"
    "$RIG_DOGSLED" --json state jobs > "$RIG/at-close/jobs.json"
    if test -f "$RIG/fail-close"; then echo close-refused >&2; exit 1; fi
    printf '{}\n' ;;
  'agent start'*|'agent prompt'*|'pane report-agent'*|'pane report-metadata'*) printf '{}\n' ;;
  'machine rename'*|'machine enable'*|'machine disable'*) printf '{}\n' ;;
  *) echo unexpected >&2; exit 2 ;;
esac
"#;

/// Release the shared kernel lock even when a parallel test's fork retained a copy.
struct ServiceLock(fs::File);

impl Drop for ServiceLock {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

struct Rig {
    dir: PathBuf,
    service: std::sync::Mutex<Option<ServiceLock>>,
    /// The time this rig's records were created at. It never moves, so a record and its
    /// receipt always name the same request time.
    origin_ms: u64,
}
impl Rig {
    /// Bound launch fixtures keep state paths below the macOS terminal-line budget.
    fn short(name: &str) -> Self {
        Self::new(name).shortened()
    }

    fn shortened(mut self) -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let short = PathBuf::from("/tmp").join(format!(
            "ds3-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        assert!(!short.exists(), "{}", short.display());
        fs::rename(&self.dir, &short).unwrap();
        self.dir = short.canonicalize().unwrap();
        self
    }

    /// A rig's sockets live in it, and Darwin limits a socket path to 104 bytes, which a
    /// macOS `$TMPDIR` alone nearly fills. `/tmp` keeps every rig socket short.
    fn directory(name: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let name: String = name.chars().take(16).collect();
        let dir = PathBuf::from("/tmp").join(format!(
            "dc-{}-{}-{name}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&dir);
        for child in ["bin", "state", "home"] {
            fs::create_dir_all(dir.join(child)).unwrap();
        }
        dir.canonicalize().unwrap()
    }

    fn new(name: &str) -> Self {
        let dir = Self::directory(name);
        let rig = Self {
            dir,
            service: std::sync::Mutex::new(None),
            origin_ms: now(),
        };
        rig.put("machines.json", &json!([]));
        rig.put("agents.json", &json!({"result":{"agents":[]}}));
        rig.put(
            "panes.json",
            &json!({"result":{"panes":[{"pane_id":"w1:p1","terminal_id":"term_owner"},{"pane_id":"w2:p1","terminal_id":"term_child"}]}}),
        );
        rig.script("herdr", HERDR_SCRIPT);
        rig.script("gh", "#!/bin/sh\nprintf '[]\\n'\n");
        rig.script(
            "ssh",
            r#"#!/bin/sh
printf '%s\n' "$*" >> "$RIG/ssh.log"
case "$*" in
  *'host snapshot'*)
    if test -f "$RIG/unreachable"; then echo offline >&2; exit 255; fi
    cat "$RIG/remote.json" ;;
  *'host reserve'*)
    for remote do :; done
    sh -c "$RIG/bin/peer-fixture $remote" ;;
  *'host checkout'*) printf '%s\n' '"/remote/repo"' ;;
  *'host launch'*)
    if test -f "$RIG/remote-binding.json"; then
      for remote do :; done
      sh -c "$RIG/bin/peer-fixture $remote"
    else
      printf '%s\n' '{"id":"remote-job","host":"8103","pane":"w9:p1","name":"worker"}'
    fi ;;
  *'host placement-finished'*) printf 'true\n' ;;
  *'host jobs'*) cat "$RIG/jobs.json" ;;
  *'host job '*) cat "$RIG/job.json" ;;
  *'host logs'*) cat "$RIG/logs.json" ;;
  *'host stop'*) printf '{}\n' ;;
  *'host prepared'*)
    for remote do :; done
    DOGSLED_STATE="$RIG/state" sh -c "$RIG/bin/peer-fixture $remote" ;;
  *'host prepare'*)
    for remote do :; done
    DOGSLED_STATE="$RIG/state" sh -c "$RIG/bin/peer-fixture $remote" ;;
  *) echo unexpected >&2; exit 2 ;;
esac
"#,
        );
        rig.script("peer-fixture", r#"#!/bin/sh
while test "$1" != host; do shift; done
shift
operation=$1; shift
while test "$#" -gt 0; do
  case "$1" in
    --request-id) id=$2; shift ;;
    --branch) branch=$2; shift ;;
    --base) base=$2; shift ;;
    --input) input=$2; shift ;;
    --request) request=$2; shift ;;
    --id) id=$2; shift ;;
    --placement-replay) placement=$2; shift ;;
  esac
  shift
done
case "$operation" in
  reserve)
    if test -z "$placement"; then cat "$RIG/reserve.json"; exit; fi
    # The fixture echoes the complete wire expectation, as the real host does.
    python3 - "$placement" "$RIG/reserve.json" <<'PY'
import json, sys
request = json.loads(sys.argv[1])
with open(sys.argv[2]) as f:
    reply = json.load(f)
grant = None
if reply['reserved']:
    grant = {'version': 3, 'envelope': {k: v for k, v in request.items() if k != 'expected_grant'},
             'reservation': {'id': request['reservation'], 'created_ms': 1},
             'headroom': {'host': request['host']['id'], 'headroom': 1.0, 'eligible': True,
                          'live_agents': 0, 'reasons': []}, 'materialized': True}
reply['placement'] = {'envelope': {k: v for k, v in request.items() if k != 'expected_grant'}, 'grant': grant}
print(json.dumps(reply))
PY
    ;;
  prepared) printf '%s,"released":true}\n' "${input%\}}" ;;
  prepare)
    prepared='{"pane_id":"w9:p1","terminal_id":"term_remote","path":"/remote/tree"}'
    if test -n "$base"; then base="\"$base\""; else base=null; fi
    printf '{"request_id":"%s","request":["/remote/repo","%s",%s],"claim":{"request_id":"%s","tree":{"repo":"/remote/repo","path":"/remote/tree","branch":"%s","head":"head","identity":{"common":"/remote/repo/.git","gitdir":"/remote/repo/.git/worktrees/tree","generation":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}}},"prepared":%s}' "$id" "$branch" "$base" "$id" "$branch" "$prepared" > "$RIG/remote-binding.json"
    printf '%s,"binding":%s}\n' "${prepared%\}}" "$(cat "$RIG/remote-binding.json")" ;;
  launch)
    printf '{"result":{"id":"remote-job","host":"8103","pane":"w9:p1","name":"worker"},"disposition":{"version":1,"binding":%s,"policy":{"kind":"transfer","launch":%s,"pane":"w9:p1","job":null},"outcome":"possibly_executed"}}\n' "$(cat "$RIG/remote-binding.json")" "$request" ;;
esac
"#);
        rig.put("reserve.json", &json!({"reserved":true,"headroom":null}));
        rig
    }
    /// Start a real observer explicitly. Background owns termination and reaping, including
    /// unwinding tests. Callers wait for their specific mutation before subsequent reads.
    fn serve(&self) -> Background {
        self.serve_at(&self.dir.join("state"), "1")
    }

    fn serve_at(&self, state: &Path, interval: &str) -> Background {
        let mut command = self.cmd(&["serve", "--interval", interval]);
        command.env("DOGSLED_STATE", state);
        let observer = Self::background(command);
        wait_publication_at(self, state, |frame| frame["revision"].is_u64());
        observer
    }

    fn service(&self) {
        let mut service = self.service.lock().unwrap();
        if service.is_none() {
            let lock = fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .open(self.dir.join("state/service.lock"))
                .unwrap();
            lock.try_lock().unwrap();
            *service = Some(ServiceLock(lock));
        }
    }
    fn put(&self, path: &str, value: &Value) {
        // Production publishes request authority before either a job or its receipt.
        // A fixture for either record must include that earlier publication too.
        if path.starts_with("state/launches/")
            && let Some(validity) = value.pointer("/request/validity").filter(|v| !v.is_null())
        {
            let id = Path::new(path)
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .split('.')
                .next()
                .unwrap();
            if self.validity(id).is_null() {
                self.put_validity(id, validity);
            }
        }
        fs::write(
            self.dir.join(path),
            serde_json::to_vec_pretty(value).unwrap(),
        )
        .unwrap();
    }
    fn disposition_request(&self, id: &str) -> String {
        let receipt = self.preparation_record("state", "receipt", id).read();
        json!({"version":1,"binding":receipt["binding"],
            "policy":{"kind":"prohibit","basis":"placement_returned"},"outcome":"unknown"})
        .to_string()
    }

    /// An immutable host outcome for identity-bearing preparation fixtures.
    fn preparation_outcome(&self, id: &str, tree: &Value, prepared: &Value) {
        assert!(tree["identity"].is_object());
        let request = json!([tree["repo"], tree["branch"], null]);
        let binding = json!({"request_id":id,"request":request,"claim":{"tree":tree,"request_id":id},"prepared":prepared});
        self.preparation_record("state", "receipt", id).put(
            &json!({"version":1,"request":request,"prepared":prepared,"failure":null,"binding":binding}),
        );
    }

    fn script(&self, name: &str, text: &str) {
        let path = self.dir.join("bin").join(name);
        let temporary = path.with_extension("next");
        fs::write(&temporary, text).unwrap();
        fs::set_permissions(&temporary, fs::Permissions::from_mode(0o755)).unwrap();
        fs::rename(temporary, path).unwrap();
    }

    fn config(&self, text: &str) {
        fs::write(self.dir.join("config.toml"), text).unwrap();
    }

    fn cmd(&self, args: &[&str]) -> Command {
        if args.first() == Some(&"serve") {
            self.service.lock().unwrap().take();
        }
        let mut cmd = isolated(env!("CARGO_BIN_EXE_dogsled"));
        cmd.env("DOGSLED_BIN", env!("CARGO_BIN_EXE_dogsled"));
        cmd.env(
            "DOGSLED_TEST_ROLE",
            if args.first() == Some(&"serve")
                || args.windows(2).any(|pair| pair == ["state", "project"])
            {
                "observer"
            } else {
                "actor"
            },
        );
        cmd.args(args);
        if args.windows(2).any(|w| w == ["host", "reserve"]) && !args.contains(&"--owner") {
            cmd.args(["--owner", "w1:p1", "--owner-host", "origin"]);
        }
        // Fixtures speak the current wire unless a test explicitly selects another version.
        if args.windows(2).any(|w| {
            w[0] == "host" && matches!(w[1], "reserve" | "prepare" | "launch" | "prepared")
        }) && !args.contains(&"--preparation-protocol")
        {
            cmd.args(["--preparation-protocol", "1"]);
        }
        if args.windows(2).any(|w| w == ["host", "prepare"])
            && args.contains(&"--request-id")
            && !args.contains(&"--preparation-envelope-version")
        {
            cmd.args([
                "--preparation-envelope-version",
                "2",
                "--request-created-ms",
                &self.origin_ms.to_string(),
            ]);
        }
        if args.windows(2).any(|w| w == ["host", "launch"])
            && !args.contains(&"--request-validity")
            && args
                .windows(2)
                .find(|w| w[0] == "--request")
                .and_then(|w| serde_json::from_str::<Value>(w[1]).ok())
                .is_some_and(|request| !request["validity"].is_null())
        {
            cmd.args(["--request-validity", "1"]);
        }
        self.rig_env(&mut cmd);
        cmd
    }

    /// The environment every process in this rig runs with. A process the rig starts, and
    /// a process the test spawns from what the rig recorded, run with the same
    /// environment, so both see the same fake Herdr, the same state, and the same rig.
    fn rig_env(&self, cmd: &mut Command) {
        cmd.env(
            "PATH",
            format!(
                "{}:{}",
                self.dir.join("bin").display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )
        .current_dir(self.dir.join("home"))
        .env("HERDR_PANE_ID", "w1:p1")
        .env("HERDR_WORKSPACE_ID", "w1")
        // Fake Herdr owns this rig; never discover or subscribe to the developer's daemon.
        .env("HERDR_SOCKET_PATH", self.dir.join("herdr.sock"))
        .env("RIG", &self.dir)
        .env("RIG_DOGSLED", env!("CARGO_BIN_EXE_dogsled"))
        .env("HOME", self.dir.join("home"))
        .env("DOGSLED_BINARY", env!("CARGO_BIN_EXE_dogsled"))
        .env("DOGSLED_CONFIG", self.dir.join("config.toml"))
        .env("DOGSLED_STATE", self.dir.join("state"));
        if let Ok(now) = fs::read_to_string(self.dir.join("clock")) {
            cmd.env("DOGSLED_TEST_NOW_MS", now.trim());
        }
        if let Ok(window) = fs::read_to_string(self.dir.join("busy-window-ms")) {
            cmd.env("DOGSLED_TEST_BUSY_WINDOW_MS", window.trim());
        }
    }

    fn run(&self, args: &[&str]) -> Output {
        self.cmd(args).output().unwrap()
    }

    /// Spawn the wrapper the fake Herdr recorded as `path`, as this test's own child.
    /// The child leads a process group of its own, so the guard kills the wrapper and
    /// every command that shares its group. The test reaps the child and reads its exit
    /// status, so the wrapper's end is proved.
    fn spawn_recorded(&self, path: &Path, what: &str) -> Owned {
        let command = read_command(path, what);
        let mut cmd = isolated("sh");
        cmd.args(["-c", &command]);
        self.rig_env(&mut cmd);
        // The wrapper's output goes to a file, so a full pipe never blocks the wrapper.
        cmd.stdout(fs::File::create(self.dir.join("wrapper-output")).unwrap());
        cmd.stderr(fs::File::create(self.dir.join("wrapper-error")).unwrap());
        Owned::spawn(&mut cmd)
    }

    /// Run `args` in the background, so the test can act while the command waits. The
    /// command is the test's own child, so the test reaps it and reads its exit status.
    /// The command leads a process group of its own, so the guard kills the commands it
    /// started too.
    fn start(&self, args: &[&str]) -> Background {
        Self::background(self.cmd(args))
    }

    fn background(mut command: Command) -> Background {
        command.stdout(Stdio::piped());
        command.stderr(Stdio::piped());
        let mut owned = Owned::spawn(&mut command);
        let mut stdout = owned.child_mut().stdout.take().unwrap();
        let mut stderr = owned.child_mut().stderr.take().unwrap();
        let (send, output) = std::sync::mpsc::channel();
        let errors = std::thread::spawn(move || {
            use std::io::Read as _;
            let mut err = Vec::new();
            let _ = stderr.read_to_end(&mut err);
            err
        });
        let reader = std::thread::spawn(move || {
            use std::io::Read as _;
            let mut out = Vec::new();
            let _ = stdout.read_to_end(&mut out);
            let err = errors.join().unwrap();
            let _ = send.send((
                String::from_utf8_lossy(&out).into_owned(),
                String::from_utf8_lossy(&err).into_owned(),
            ));
        });
        Background {
            owned,
            reader: Some(reader),
            output,
        }
    }

    fn json(&self, args: &[&str]) -> (i32, Value) {
        let out = self.cmd(args).arg("--json").output().unwrap();
        let value: Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
            panic!(
                "{e}: {} {}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            )
        });
        // Direct-host fixtures compare the immutable result, not its transport envelope.
        let value = if args.windows(2).any(|w| w == ["host", "launch"])
            && value.get("disposition").is_some()
        {
            value["result"].clone()
        } else {
            value
        };
        (out.status.code().unwrap(), value)
    }

    fn snapshot(&self, path: &str, sampled: u64, load: f64, cores: u32) {
        // Busy fraction that reproduces the old load rule's verdict for this load. The zero
        // tick counters let a Linux `serve --once` derive a busy sample from one real reading.
        let busy = (load / (1.5 * f64::from(cores))).min(1.0);
        let snapshot = json!({"placement_replay":3,"request_validity":1,"sampled_ms":sampled,"probe":{
            "host":"local","label":"local","load1":load,"cores":cores,"busy":busy,
            "cpu_ticks":{"busy":0,"total":0},
            "memory":{"used_bytes":1_073_741_824_u64,"total_bytes":8_589_934_592_u64},
            "disk_free_percent":50.0,"agents":{"total":1,"by_status":{"idle":1}},
            "agent_panes":[{"pane":"w1:p1","agent":"pi","state":"idle","name":null,"transcript":null,"transcript_modified":null}],"reservations":[],"panes":[]
        }});
        if Path::new(path)
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
        {
            self.put(path, &snapshot);
        } else {
            self.put_host_at(path, &snapshot);
        }
    }

    /// Branch/refusal fixtures need stable capacity, not the runner's resources or clock.
    fn placement_capacity(&self) {
        // The snapshot template supplies 7 GiB free memory and 50% free disk.
        self.snapshot("state", self.now_ms(), 0.0, 16);
        fs::write(
            self.dir.join("clock"),
            self.host()["sampled_ms"].to_string(),
        )
        .unwrap();
    }

    /// Explicit observer peer ingestion without replacing the local fixture sample.
    /// The failed local probe publishes a gap and exits 2, after peer ingestion.
    fn ingest_peers(&self) {
        let running = self.service.lock().unwrap().is_some();
        let unavailable = self.dir.join("unavailable-origin-agents.json");
        fs::write(&unavailable, "unavailable local probe").unwrap();
        let output = self
            .cmd(&["serve", "--once", "--interval", "1"])
            .env("RIG_AGENTS", unavailable)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2), "{output:?}");
        if running {
            self.service();
        }
    }

    fn peer(&self) {
        self.put(
            "machines.json",
            &json!([{"id":"8103","label":"netcup","target":"netcup-dev"}]),
        );
        self.snapshot("remote.json", now(), 3.0, 16);
    }

    /// The agent in `terminal` called `dogsled done`, and the turn that called it ended as
    /// completion `seq`.
    fn done(&self, terminal: &str, pane: &str, seq: u64) {
        self.put_done(&json!({terminal: {"pane":pane,"after":seq - 1,"sealed":seq,"marked_ms":1}}));
    }

    fn database(&self) -> rusqlite::Connection {
        fs::create_dir_all(self.dir.join("state")).unwrap();
        let mut connection = rusqlite::Connection::open(self.dir.join("state/state.db")).unwrap();
        connection.busy_timeout(Duration::from_secs(30)).unwrap();
        let initialization = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(self.dir.join("state/state-db.lock"))
            .unwrap();
        initialization.lock().unwrap();
        connection
            .query_row("PRAGMA journal_mode=WAL", [], |_| Ok(()))
            .unwrap();
        let tx = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .unwrap();
        let version: u32 = tx
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        if version == 0 {
            let schema = self.run(&["state", "schema"]);
            assert!(schema.status.success(), "{schema:?}");
            tx.execute_batch(&String::from_utf8(schema.stdout).unwrap())
                .unwrap();
        }
        tx.commit().unwrap();
        initialization.unlock().unwrap();
        connection
    }

    fn items(&self) -> Value {
        self.state(&["items"])
    }

    fn tickets(&self) -> Value {
        self.state(&["tickets"])
    }

    fn put_done(&self, value: &Value) {
        let mut marks = value.clone();
        let agents = self.read("agents.json");
        let items = self.items();
        for mark in marks.as_object_mut().unwrap().values_mut() {
            let pane = mark["pane"].as_str().unwrap().to_owned();
            if let Some(agent) = agents["result"]["agents"]
                .as_array()
                .unwrap()
                .iter()
                .find(|agent| agent["pane_id"] == pane)
            {
                if mark.get("session").is_none() {
                    mark["session"] = agent["agent_session"]["value"].clone();
                }
                if mark.get("name").is_none() {
                    mark["name"] = agent["name"].clone();
                }
            }
            if mark.get("item").is_none()
                && let Some(item) = items
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|item| item["pane"] == pane && item["closed_ms"].is_null())
            {
                mark["item"] = item["id"].clone();
            }
            if mark.get("item").is_none() {
                mark["item"] = self.state(&["done-identity", &pane]);
            }
        }
        self.put_state(&["state", "done", "--put"], &marks);
    }

    fn fault_host_store(&self, store: &str, write: bool) {
        let mut args = vec!["fault", store];
        if write {
            args.push("--write");
        }
        self.state(&args);
    }

    fn host(&self) -> Value {
        self.state(&["host"])
    }

    fn host_at(&self, state: &str) -> Value {
        self.state_at(state, &["host"])
    }

    fn put_host(&self, value: &Value) {
        self.put_host_at("state", value);
    }

    fn put_host_at(&self, state: &str, value: &Value) {
        if state == "state" {
            self.service();
        }
        self.put_state_in(&["state", "host", "--put"], value, state);
    }

    fn launch_record(&self, id: &str, record: &str) -> Value {
        self.state(&["launch", id, "--record", record])
    }

    fn put_launch_record(&self, id: &str, record: &str, value: &Value) {
        if let Some(validity) = value.pointer("/request/validity").filter(|v| !v.is_null())
            && self.validity(id).is_null()
        {
            self.put_validity(id, validity);
        }
        self.put_state(&["state", "launch", id, "--record", record, "--put"], value);
    }

    fn launch(&self, id: &str) -> Value {
        self.launch_record(id, "receipt")
    }

    fn put_launch(&self, id: &str, value: &Value) {
        self.put_launch_record(id, "receipt", value);
    }

    fn launch_progress(&self, id: &str) -> Value {
        self.launch_record(id, "progress")
    }

    fn put_launch_progress(&self, id: &str, value: &Value) {
        self.put_launch_record(id, "progress", value);
    }

    fn launch_seed(&self, id: &str) -> Value {
        self.launch_record(id, "seed")
    }

    fn put_launch_seed(&self, id: &str, value: &Value) {
        self.put_launch_record(id, "seed", value);
    }

    fn launch_continuations(&self, id: &str) -> Value {
        self.launch_record(id, "continuations")
    }

    fn put_launch_continuations(&self, id: &str, value: &Value) {
        self.put_launch_record(id, "continuations", value);
    }

    fn delete_launch_record(&self, id: &str, record: &str) {
        self.state(&["launch", id, "--record", record, "--delete"]);
    }

    fn refuse_launch_record(&self, id: &str, record: &str, value: &Value) {
        let before = self.launch_record(id, record);
        let mut child = self
            .cmd(&["state", "launch", id, "--record", record, "--put"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        std::io::Write::write_all(
            &mut child.stdin.take().unwrap(),
            value.to_string().as_bytes(),
        )
        .unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(
            !out.status.success(),
            "malformed {record} was stored: {out:?}"
        );
        assert_eq!(self.launch_record(id, record), before);
    }

    fn launch_ids(&self) -> Vec<String> {
        serde_json::from_value(self.state(&["launches"])).unwrap()
    }

    fn peer_launch_record(&self, id: &str, record: &str) -> Value {
        let out = self
            .cmd(&["state", "launch", id, "--record", record, "--json"])
            .env("DOGSLED_STATE", self.dir.join("peer-state"))
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        serde_json::from_slice(&out.stdout).unwrap()
    }

    fn put_peer_launch_record(&self, id: &str, record: &str, value: &Value) {
        let mut child = self
            .cmd(&["state", "launch", id, "--record", record, "--put"])
            .env("DOGSLED_STATE", self.dir.join("peer-state"))
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        std::io::Write::write_all(
            &mut child.stdin.take().unwrap(),
            value.to_string().as_bytes(),
        )
        .unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success(), "{out:?}");
    }

    /// The admission stored for request `id`, or null.
    fn admission(&self, id: &str) -> Value {
        self.state(&["admission", id])
    }

    fn delete_admission(&self, id: &str) {
        self.state(&["admission", id, "--delete"]);
    }

    fn admission_ids(&self) -> Vec<String> {
        self.admissions()
            .as_array()
            .unwrap()
            .iter()
            .map(|attempt| attempt["request"]["id"].as_str().unwrap().to_owned())
            .collect()
    }

    fn put_preparation_refused(&self, state: &str, kind: &str, id: &str, value: &Value) -> Output {
        let mut child = self
            .cmd(&["state", "preparation", kind, id, "--put"])
            .env("DOGSLED_STATE", self.dir.join(state))
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        std::io::Write::write_all(
            &mut child.stdin.take().unwrap(),
            value.to_string().as_bytes(),
        )
        .unwrap();
        child.wait_with_output().unwrap()
    }

    fn preparations(&self) -> Value {
        self.preparation_record("state", "claims", "").read()
    }

    fn preparation_record<'a>(
        &'a self,
        state: &'a str,
        kind: &'a str,
        id: &str,
    ) -> PreparationRecord<'a> {
        PreparationRecord {
            rig: self,
            state,
            kind,
            id: id.into(),
        }
    }

    /// Run `dogsled state` with `args`, and return its JSON output.
    fn state(&self, args: &[&str]) -> Value {
        self.state_at("state", args)
    }

    fn state_at(&self, state: &str, args: &[&str]) -> Value {
        let out = self
            .cmd(&[&["state"], args].concat())
            .env("DOGSLED_STATE", self.dir.join(state))
            .arg("--json")
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        serde_json::from_slice(&out.stdout).unwrap_or(Value::Null)
    }

    /// Make every later admission update fail, as a storage failure would.
    fn refuse_admission_writes(&self) {
        self.database()
            .execute_batch(
                "CREATE TRIGGER refuse_admission_writes BEFORE UPDATE ON admissions
                 BEGIN SELECT RAISE(ABORT, 'admission write refused'); END;",
            )
            .unwrap();
    }

    fn has_admission(&self, id: &str) -> bool {
        !self.admission(id).is_null()
    }

    /// Every stored admission, oldest first.
    fn admissions(&self) -> Value {
        self.state(&["admissions"])
    }

    fn put_admission(&self, id: &str, value: &Value) {
        self.put_state(&["state", "admission", id, "--put"], value);
    }

    fn inbox(&self, terminal: &str) -> Value {
        self.state(&["inbox", terminal])
    }

    fn put_inbox(&self, terminal: &str, records: &Value) {
        self.put_state(&["state", "inbox", terminal, "--put"], records);
    }

    fn put_inbox_reader(&self, terminal: &str, reader: &Value) {
        self.put_state(&["state", "inbox-reader", terminal, "--put"], reader);
    }

    fn delete_inbox_reader(&self, terminal: &str) {
        self.state(&["inbox-reader", terminal, "--delete"]);
    }

    fn outbox(&self) -> Value {
        self.state(&["outbox"])
    }

    fn clear_outbox(&self) {
        self.state(&["outbox", "--delete"]);
    }

    fn remote_inbox(&self, terminal: &str) -> Value {
        let out = self
            .cmd(&["state", "inbox", terminal])
            .env("DOGSLED_STATE", self.dir.join("remote-state"))
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        serde_json::from_slice(&out.stdout).unwrap()
    }

    fn put_remote_state(&self, args: &[&str], value: &Value) {
        let mut child = self
            .cmd(args)
            .env("DOGSLED_STATE", self.dir.join("remote-state"))
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        std::io::Write::write_all(
            &mut child.stdin.take().unwrap(),
            value.to_string().as_bytes(),
        )
        .unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success(), "{out:?}");
    }

    /// Run `dogsled` with `args`, with `value` as its JSON input.
    fn put_state(&self, args: &[&str], value: &Value) {
        self.put_state_in(args, value, "state");
    }

    /// Run `dogsled` with `args` and `value` as its JSON input, and return its output.
    fn write_state(&self, args: &[&str], value: &Value) -> Output {
        self.write_state_in(args, value, "state")
    }

    fn put_state_in(&self, args: &[&str], value: &Value, state: &str) {
        let out = self.write_state_in(args, value, state);
        assert!(out.status.success(), "{args:?}: {out:?}");
    }

    fn write_state_in(&self, args: &[&str], value: &Value, state: &str) -> Output {
        let mut child = self
            .cmd(args)
            .env("DOGSLED_STATE", self.dir.join(state))
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        std::io::Write::write_all(
            &mut child.stdin.take().unwrap(),
            value.to_string().as_bytes(),
        )
        .unwrap();
        child.wait_with_output().unwrap()
    }

    fn job(&self, id: &str) -> Value {
        self.state(&["job", id])
    }

    fn stop_requested(&self, id: &str) -> Value {
        self.state(&["job-stop", id])
    }

    fn at_close(&self, id: &str) -> Value {
        self.read("at-close/jobs.json")
            .as_array()
            .unwrap()
            .iter()
            .find(|job| job["id"] == id)
            .unwrap()
            .clone()
    }

    fn validity(&self, id: &str) -> Value {
        self.state(&["validity", id])
    }

    fn cancellation(&self, id: &str) -> Value {
        self.state(&["cancellation", id])
    }

    fn clock(&self) -> Value {
        self.state(&["clock"])
    }

    fn reservations(&self) -> Value {
        self.state(&["reservations"])
    }

    fn put_job(&self, id: &str, value: &Value) {
        if let Some(validity) = value.get("validity").filter(|v| !v.is_null())
            && self.validity(id).is_null()
        {
            self.put_validity(id, validity);
        }
        self.put_state(&["state", "job", id, "--put"], value);
    }

    fn put_validity(&self, id: &str, value: &Value) {
        self.put_state(&["state", "validity", id, "--put"], value);
    }

    /// Caller provenance normally published by the reserve ingress.
    fn put_request_owner(&self, id: &str, owner: &str, host: &str) {
        assert_eq!(
            self.database()
                .execute(
                    "UPDATE job_validity SET owner = ?2, owner_host = ?3 WHERE id = ?1",
                    rusqlite::params![id, owner, host],
                )
                .unwrap(),
            1
        );
    }

    fn put_cancellation(&self, id: &str, value: &Value) {
        self.put_state(&["state", "cancellation", id, "--put"], value);
    }

    fn put_clock(&self, value: &Value) {
        self.put_state(&["state", "clock", "--put"], value);
    }

    fn put_reservations(&self, value: &Value) {
        self.put_state(&["state", "reservations", "--put"], value);
    }

    fn delete_job(&self, id: &str) {
        self.state(&["job", id, "--delete"]);
    }

    fn delete_validity(&self, id: &str) {
        self.state(&["validity", id, "--delete"]);
    }

    fn peer_state(&self, args: &[&str]) -> Value {
        let out = self
            .cmd(&[&["state"], args].concat())
            .env("DOGSLED_STATE", self.dir.join("peer-state"))
            .arg("--json")
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        serde_json::from_slice(&out.stdout).unwrap_or(Value::Null)
    }

    fn put_peer_state(&self, args: &[&str], value: &Value) {
        let mut child = self
            .cmd(&[&["state"], args].concat())
            .env("DOGSLED_STATE", self.dir.join("peer-state"))
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        std::io::Write::write_all(
            &mut child.stdin.take().unwrap(),
            value.to_string().as_bytes(),
        )
        .unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success(), "{out:?}");
    }
    fn put_items(&self, value: &Value) {
        self.put_state(&["state", "items", "--put"], value);
    }
    fn put_tickets(&self, value: &Value) {
        self.put_state(&["state", "tickets", "--put"], value);
    }
    fn read(&self, path: &str) -> Value {
        serde_json::from_slice(&fs::read(self.dir.join(path)).unwrap()).unwrap()
    }
    /// A job claim that a wrapper may run: the command, the options, and a pane.
    /// A wrapper runs only from a claim like this one.
    fn claim(&self, id: &str, cmd: &[&str]) {
        self.claim_with(id, cmd, &json!(null), &json!(null), "w1:p1");
    }
    /// A claim created at `created_ms`, so a request of a given age can be tested.
    fn claim_at(&self, id: &str, cmd: &[&str], created_ms: u64) {
        self.claim_validity(id, cmd, &json!(null), &json!(null), "w1:p1", created_ms);
    }
    /// A claim with the job's own timeout options, so a request that names different ones
    /// asks for a different job.
    fn claim_with(
        &self,
        id: &str,
        cmd: &[&str],
        timeout_ms: &Value,
        idle_timeout_ms: &Value,
        pane: &str,
    ) {
        self.claim_validity(id, cmd, timeout_ms, idle_timeout_ms, pane, self.now_ms());
    }
    /// A claim with the job's own timeout options and the request's own creation time.
    fn claim_validity(
        &self,
        id: &str,
        cmd: &[&str],
        timeout_ms: &Value,
        idle_timeout_ms: &Value,
        pane: &str,
        created_ms: u64,
    ) {
        let job = json!({
            "id": id,
            "cmd": cmd,
            "cwd": self.dir.join("home").display().to_string(),
            "host": "local",
            "owner": "w1:p1",
            "pane": pane,
            "state": "pending",
            "exit": Value::Null,
            "signal": Value::Null,
            "reason": Value::Null,
            "started_ms": Value::Null,
            "ended_ms": Value::Null,
            "timeout_ms": timeout_ms,
            "idle_timeout_ms": idle_timeout_ms,
            "validity": {"version": 1, "created_ms": created_ms},
            "startup": {"version":1,"scope":"non_repository","cwd":self.dir.join("home")},
        });
        self.put_job(id, &job);
    }
    /// The validity evidence a record of this rig carries: the request's own creation
    /// time, which never moves while the rig lives.
    fn job_validity(&self) -> Value {
        json!({"version": 1, "created_ms": self.origin_ms})
    }
    /// The host clock this rig runs at, or the real clock when the rig sets none.
    fn now_ms(&self) -> u64 {
        fs::read_to_string(self.dir.join("clock"))
            .ok()
            .and_then(|text| text.trim().parse().ok())
            .unwrap_or_else(now)
    }
    fn log(&self, path: &str) -> String {
        fs::read_to_string(self.dir.join(path)).unwrap_or_default()
    }
    fn expire_cache(&self) {
        let mut cache = self.state(&["peer", "8103"]);
        cache["fetched_ms"] = json!(0);
        cache["retry"] = json!({});
        self.put_state(&["state", "peer", "8103", "--put"], &cache);
    }
}
impl Drop for Rig {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn rig_releases_service_lock_while_an_inherited_handle_remains_open() {
    let rig = Rig::new("inherited-service-lock");
    rig.service();
    // A fork retains the same open file description until exec; a clone models it exactly.
    let inherited = rig
        .service
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .0
        .try_clone()
        .unwrap();
    rig.service.lock().unwrap().take();
    rig.service();
    drop(inherited);
}

#[test]
fn serve_once_publishes_local_facts_and_discovers_peers_once_without_ssh() {
    let rig = Rig::new("serve");
    rig.put(
        "agents.json",
        &json!({"result":{"agents":[{"pane_id":"w1:p2","agent_status":"working"}]}}),
    );
    let out = rig.run(&["serve", "--once", "--interval", "1"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let snapshot = rig.host();
    assert!(snapshot["sampled_ms"].as_u64().unwrap() > 0);
    assert!(snapshot["probe"]["memory"]["total_bytes"].as_u64().unwrap() > 0);
    assert_eq!(snapshot["probe"]["agents"]["total"], 1);
    assert_eq!(snapshot["probe"]["agent_panes"][0]["pane"], "w1:p2");
    assert_eq!(snapshot["probe"]["panes"][0]["pane_id"], "w1:p1");
    // The shell pane is at an empty prompt from this sample on.
    assert_eq!(snapshot["probe"]["panes"][0]["idle_shell"], true);
    assert!(
        snapshot["probe"]["panes"][0]["idle_since_ms"]
            .as_u64()
            .unwrap()
            <= snapshot["sampled_ms"].as_u64().unwrap()
    );
    // `--once` keeps publishing while its action lanes run, so a slow host can
    // add ticks. Every tick reads agents, panes, and each shell's foreground.
    // No Stop request means no doctor survey or workspace list.
    let herdr = rig.log("herdr.log");
    assert_eq!(
        herdr
            .lines()
            .filter(|line| *line == "machine list --json")
            .count(),
        1
    );
    // The event subscription asks Herdr for its socket; the fixture has none, so serve
    // keeps its fallback timer. That lookup is not a collection tick.
    let herdr = herdr
        .lines()
        .filter(|line| !["machine list --json", "status server --json"].contains(line))
        .fold(String::new(), |mut text, line| {
            text.push_str(line);
            text.push('\n');
            text
        });
    let ticks: Vec<_> = herdr.split("agent list\n").collect();
    assert_eq!(ticks[0], "");
    assert!(ticks.len() > 1);
    for tick in &ticks[1..] {
        let mut calls = tick.lines();
        assert_eq!(calls.next(), Some("pane list"));
        let mut foreground: Vec<_> = calls.collect();
        foreground.sort_unstable();
        assert_eq!(
            foreground,
            [
                "pane process-info --pane w1:p1",
                "pane process-info --pane w2:p1"
            ]
        );
    }
    assert!(
        herdr.lines().all(|line| [
            "agent list",
            "pane list",
            "pane process-info --pane w1:p1",
            "pane process-info --pane w2:p1"
        ]
        .contains(&line)),
        "{herdr}"
    );
    assert_eq!(rig.log("ssh.log"), "");
    assert!(
        !fs::read_dir(rig.dir.join("state")).unwrap().any(|e| e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .ends_with(".tmp"))
    );
    assert!(
        !rig.run(&["serve", "--once", "--interval", "0"])
            .status
            .success()
    );
}

#[test]
fn failed_probe_preserves_last_complete_snapshot() {
    let rig = Rig::new("failed-probe");
    rig.snapshot("state", 1, 2.0, 4);
    let before = rig.host();
    rig.script("herdr", "#!/bin/sh\necho disconnected >&2\nexit 1\n");
    assert_eq!(rig.run(&["serve", "--once"]).status.code(), Some(2));
    assert_eq!(rig.host(), before);
}

#[test]
fn serve_records_transcript_paths_and_file_mtimes() {
    let rig = Rig::new("transcripts");
    let transcript = rig.dir.join("session.jsonl");
    fs::write(&transcript, "{\"message\":\"hello\"}\n").unwrap();
    let modified_ms = 1_700_000_000_123_u64;
    fs::File::open(&transcript)
        .unwrap()
        .set_modified(UNIX_EPOCH + std::time::Duration::from_millis(modified_ms))
        .unwrap();
    rig.put("agents.json", &json!({"result":{"agents":[
        {"pane_id":"w1:p1","agent":"pi","agent_status":"working","agent_session":{"kind":"path","value":transcript}},
        {"pane_id":"w1:p2","agent":"codex","agent_status":"idle","agent_session":{"kind":"id","value":"session-id"}},
        {"pane_id":"w1:p3","agent":"claude","agent_session":{"kind":"path","value":"/missing/transcript"}},
        {"pane_id":"w1:p4","agent":"pi"}
    ]}}));
    assert!(rig.run(&["serve", "--once"]).status.success());
    let snapshot = rig.host();
    let agents = snapshot["probe"]["agent_panes"].as_array().unwrap();
    assert_eq!(agents[0]["pane"], "w1:p1");
    assert_eq!(agents[0]["agent"], "pi");
    assert_eq!(agents[0]["state"], "working");
    assert_eq!(agents[0]["transcript"], transcript.to_str().unwrap());
    assert_eq!(agents[0]["transcript_modified"], modified_ms);
    assert_eq!(agents[1]["state"], "idle");
    assert!(agents[1]["transcript"].is_null());
    assert!(agents[1]["transcript_modified"].is_null());
    assert_eq!(agents[2]["transcript"], "/missing/transcript");
    assert!(agents[2]["transcript_modified"].is_null());
    assert_eq!(agents[3]["state"], "unknown");
    assert!(agents[3]["transcript"].is_null());
}

#[test]
fn service_liveness_and_snapshot_age_are_independent() {
    let rig = Rig::new("service-liveness");
    fs::write(rig.dir.join("clock"), "61000").unwrap();
    rig.snapshot("state", 1000, 0.0, 8);
    let snapshot = rig.host();
    let report = json!({"id":"local","label":"local","target":null,"service":"up","reachable":true,"sampled_ms":1000,
        "probe":snapshot["probe"],"jobs":[],"headroom":{"host":"local","headroom":8.0,"eligible":false,"live_agents":0}});
    let mut frame = observation::publication(1);
    let mut peer = report.clone();
    peer["id"] = json!("8103");
    peer["label"] = json!("netcup");
    peer["service"] = json!("down");
    frame["views"]["status"]["data"]["hosts"] = json!([report, peer]);
    frame["views"]["status_display"]["data"] = observation::display(vec![
        observation::host_display("local", "0/8", "no_headroom", Some(1000), "service: up"),
        observation::host_display("netcup", "0/8", "unreachable", Some(1000), "service: down"),
    ]);
    frame["local"]["host"]["sampled_ms"] = json!(1000);
    frame["local"]["host"]["probe"] = snapshot["probe"].clone();
    let publisher = observation::Supplied::start(&rig, frame.clone());
    let status = rig.json(&["status"]).1;
    assert_eq!(status["hosts"][0]["service"], "up");
    assert_eq!(status["hosts"][0]["sampled_ms"], 1000);
    assert_eq!(status["hosts"][0]["headroom"]["eligible"], false);
    let text = rig.run(&["status", "--brief"]);
    assert!(String::from_utf8_lossy(&text.stdout).contains("service: up (snapshot 1m ago)"));
    let live = rig.json(&["host", "snapshot"]).1;
    assert_eq!(live["running"], true);
    assert_eq!(live["sampled_ms"], 1000);
    assert_eq!(status["hosts"][1]["service"], "down");
    assert_eq!(status["hosts"][1]["sampled_ms"], 1000);
    // A fresh, idle, roomy probe would be eligible if the reader recomputed it.
    // The publisher's deliberate refusal remains authoritative for display.
    frame["revision"] = json!(2);
    frame["views"]["status"]["data"]["hosts"][0]["sampled_ms"] = json!(61000);
    frame["local"]["host"]["running"] = json!(false);
    publisher.replace(frame);
    wait_publication(&rig, |frame| frame["revision"] == 2);
    assert_eq!(
        rig.json(&["status"]).1["hosts"][0]["headroom"]["eligible"],
        false
    );
    assert_eq!(rig.json(&["host", "snapshot"]).1["running"], false);
    drop(publisher);
    for args in [vec!["status"], vec!["host", "snapshot"]] {
        let (code, error) = rig.json(&args);
        assert_ne!(code, 0);
        assert!(
            error["error"]
                .as_str()
                .unwrap()
                .contains("observation unavailable"),
            "{error}"
        );
    }
}

#[test]
fn status_reads_snapshots_and_caches_peer_reads() {
    let rig = Rig::new("status-cache");
    rig.peer();
    rig.snapshot("state", now(), 50.0, 12);
    let local = rig.host();
    rig.config("remote_command = ['custom-dogsled', 'argument with spaces']\n");
    rig.ingest_peers();
    // Supply the captured fixtures with explicit producer display classifications.
    // The reader must not reconstruct either host from these persisted source rows.
    let peer = rig.state(&["peer", "8103"]);
    let mut frame = observation::publication(1);
    let local_report = json!({"id":"local","label":"local","target":null,"service":"up","reachable":true,"sampled_ms":local["sampled_ms"],"probe":local["probe"],"jobs":[],"headroom":{"host":"local","headroom":0.0,"eligible":false,"live_agents":1}});
    let mut peer_probe = peer["snapshot"]["probe"].clone();
    peer_probe["host"] = json!("8103");
    peer_probe["label"] = json!("netcup");
    let peer_report = json!({"id":"8103","label":"netcup","target":"netcup-dev","service":"up","reachable":true,"sampled_ms":peer["snapshot"]["sampled_ms"],"probe":peer_probe,"jobs":[],"headroom":{"host":"8103","headroom":14.0,"eligible":true,"live_agents":1}});
    frame["views"]["status"]["data"]["hosts"] = json!([local_report, peer_report]);
    let mut local_display = observation::host_display(
        "local",
        "12/12",
        "no_headroom",
        local["sampled_ms"].as_u64(),
        "service: up",
    );
    local_display["metrics"] = json!({"memory":local["probe"]["memory"],"disk_free_percent":50.0});
    local_display["states"] = json!({"stopped":1});
    local_display["agents"] =
        json!([{"pane":"w1:p1","state":"stopped","name":"","stalled_ms":null}]);
    let mut peer_display = observation::host_display(
        "netcup",
        "2/16",
        "eligible",
        peer["snapshot"]["sampled_ms"].as_u64(),
        "service: up",
    );
    peer_display["metrics"] = local_display["metrics"].clone();
    peer_display["states"] = local_display["states"].clone();
    peer_display["agents"] = local_display["agents"].clone();
    frame["views"]["status_display"]["data"] =
        observation::display(vec![local_display, peer_display]);
    let publication = observation::Supplied::start(&rig, frame);
    // Serve probed herdr above; readers must never probe it themselves.
    let probes = rig.log("herdr.log").matches("agent list").count();
    let (code, status) = rig.json(&["status"]);
    assert_eq!(code, 0);
    assert_eq!(status["hosts"].as_array().unwrap().len(), 2);
    assert_eq!(status["hosts"][0]["probe"]["load1"], 50.0);
    assert_eq!(status["hosts"][1]["probe"]["host"], "8103");
    assert_eq!(status["hosts"][1]["service"], "up");
    let line = rig.run(&["status", "--line"]);
    assert_eq!(
        String::from_utf8(line.stdout).unwrap(),
        "local 12/12 ▲ · netcup 2/16\n"
    );
    let brief = String::from_utf8(rig.run(&["status", "--brief"]).stdout).unwrap();
    assert_eq!(brief.lines().count(), 3);
    assert!(brief.contains("mem 1.0/8.0GiB disk 50% free agents 1"));
    assert!(brief.contains("Run `dogsled status`"));
    assert_eq!(rig.log("ssh.log").lines().count(), 2);
    assert!(
        rig.log("ssh.log")
            .contains("custom-dogsled 'argument with spaces' host snapshot --json")
    );
    assert_eq!(rig.log("herdr.log").matches("agent list").count(), probes);
    let reads = rig.log("ssh.log").lines().count();
    rig.expire_cache();
    rig.json(&["status"]);
    assert_eq!(
        rig.log("ssh.log").lines().count(),
        reads,
        "status never fills an expired cache"
    );
    assert_eq!(rig.log("herdr.log").matches("agent list").count(), probes);
    drop(publication);
    rig.ingest_peers();
    assert_eq!(rig.log("ssh.log").lines().count(), reads + 2);
}

fn assert_peer_retry(rig: &Rig) {
    let retry = rig.run(&["retry", "netcup", "--json"]);
    let publication = rig.run(&["observation", "current", "--json"]);
    assert_eq!(
        retry.status.code(),
        Some(0),
        "retry stdout={} stderr={} publication={}",
        String::from_utf8_lossy(&retry.stdout),
        String::from_utf8_lossy(&retry.stderr),
        String::from_utf8_lossy(&publication.stdout)
    );
}

fn assert_observation_cold(rig: &Rig) {
    let (code, cold) = rig.json(&["status"]);
    assert_ne!(code, 0);
    assert!(
        cold["error"]
            .as_str()
            .unwrap()
            .contains("observation unavailable")
    );
}

fn trap_observation_reader(rig: &Rig) {
    // Attribute access to reader processes, not to the independent live observer.
    for name in ["herdr", "ssh"] {
        let script = rig.log(&format!("bin/{name}"));
        rig.script(name, &script.replacen("#!/bin/sh", "#!/bin/sh\nif test -n \"$DOGSLED_READER_TEST\"; then printf '%s\\n' \"$*\" >> \"$RIG/reader-access.log\"; fi", 1));
    }
    rig.script("git", "#!/bin/sh\nif test -n \"$DOGSLED_READER_TEST\"; then printf git >> \"$RIG/reader-access.log\"; exit 97; fi\nexit 1\n");
}

fn observation_reader(rig: &Rig, args: &[&str]) -> std::process::Output {
    let before = rig.log("reader-access.log");
    let output = rig
        .cmd(args)
        .env("DOGSLED_READER_TEST", "1")
        .output()
        .unwrap();
    assert_eq!(rig.log("reader-access.log"), before, "{args:?}");
    output
}

fn observation_json_reader(rig: &Rig, args: &[&str]) -> (i32, Value) {
    let mut args = args.to_vec();
    args.push("--json");
    let output = observation_reader(rig, &args);
    (
        output.status.code().unwrap(),
        serde_json::from_slice::<Value>(&output.stdout).unwrap(),
    )
}

#[test]
fn missing_stale_unreachable_and_malformed_hosts_remain_visible() {
    let rig = Rig::new("down");
    assert_observation_cold(&rig);
    rig.peer();
    let sampled = now() - 31_000;
    rig.snapshot("remote.json", sampled, 6.0, 7);
    fs::write(rig.dir.join("agents.json"), "unavailable local probe").unwrap();
    trap_observation_reader(&rig);
    let read = |args: &[&str]| observation_reader(&rig, args);
    let json_read = |args: &[&str]| observation_json_reader(&rig, args);
    let observer = rig.serve();
    wait_publication(&rig, |frame| {
        frame["sources"]["peers"]["8103"]["availability"] == "stale"
            && frame["views"]["status"]["data"]["hosts"][1]["sampled_ms"] == sampled
            && frame["gaps"]
                .as_array()
                .is_some_and(|gaps| gaps.iter().any(|gap| gap["domain"] == "host"))
    });
    let (_, status) = json_read(&["status"]);
    assert_eq!(status["hosts"][0]["service"], "unavailable");
    assert!(status["hosts"][0]["probe"].is_null());
    assert!(status["hosts"][0]["sampled_ms"].is_null());
    assert!(status["hosts"][0]["headroom"].is_null());
    assert_eq!(status["hosts"][1]["service"], "down");
    assert_eq!(status["hosts"][1]["probe"]["load1"], 6.0);
    assert_eq!(status["hosts"][1]["headroom"]["eligible"], false);
    assert_eq!(
        String::from_utf8(read(&["status", "--line"]).stdout).unwrap(),
        "local ? · netcup 4/7 ?\n"
    );
    let previous = wait_publication(&rig, |_| true)["sources"]["peers"]["8103"]["received_ms"]
        .as_u64()
        .unwrap();
    fs::write(rig.dir.join("unreachable"), "").unwrap();
    assert_eq!(rig.json(&["retry", "netcup"]).0, 0);
    wait_publication(&rig, |frame| {
        frame["sources"]["peers"]["8103"]["availability"] == "unavailable"
            && frame["sources"]["peers"]["8103"]["received_ms"]
                .as_u64()
                .is_some_and(|time| time > previous)
            && frame["views"]["status"]["data"]["hosts"][1]["sampled_ms"] == sampled
    });
    let (_, status) = json_read(&["status"]);
    assert_eq!(status["hosts"][1]["reachable"], false);
    assert_eq!(status["hosts"][1]["probe"]["load1"], 6.0);
    let retry = status["hosts"][1]["retry_ms"].as_u64().unwrap();
    assert!(retry > now(), "{retry}");
    let (_, board) = json_read(&["board", "--all"]);
    assert_eq!(board["hosts"][1]["retry_ms"], retry);
    assert!(
        String::from_utf8(read(&["status", "--line"]).stdout)
            .unwrap()
            .contains("netcup 4/7 ?")
    );
    fs::remove_file(rig.dir.join("unreachable")).unwrap();
    let script = rig.log("bin/ssh");
    rig.put("peer-export.json", &json!({"version":1,"epoch":"retry-peer","local_revision":1,
        "measured":{"host":sampled},"local":{"host":serde_json::from_str::<Value>(&rig.log("remote.json")).unwrap()},"gaps":[]}));
    rig.script(
        "ssh",
        &script.replacen(
            "case \"$*\" in",
            "case \"$*\" in\n  *'host observation'*) cat \"$RIG/peer-export.json\" ;;",
            1,
        ),
    );
    let reads = rig.log("ssh.log").lines().count();
    let (code, report) = rig.json(&["retry", "netcup"]);
    assert_eq!(code, 0, "{report}");
    assert_eq!(report["reachable"], true);
    assert!(report["retry_ms"].is_null());
    assert_eq!(rig.log("ssh.log").lines().count(), reads + 1);

    rig.script("ssh", &script);
    rig.put("remote.json", &Value::Null);
    assert_peer_retry(&rig);
    wait_publication(&rig, |frame| {
        frame["views"]["status"]["data"]["hosts"][1]["error"]
            == "host snapshot is missing; service: down"
            && frame["views"]["status"]["data"]["hosts"][1]["sampled_ms"] == sampled
    });
    let (_, status) = json_read(&["status"]);
    assert_eq!(status["hosts"][1]["service"], "down");
    assert_eq!(status["hosts"][1]["probe"]["load1"], 6.0);
    fs::write(rig.dir.join("remote.json"), "bad json").unwrap();
    assert_peer_retry(&rig);
    wait_publication(&rig, |frame| {
        frame["sources"]["peers"]["8103"]["availability"] == "unavailable"
            && frame["views"]["status"]["data"]["hosts"][1]["failure"] == "unexpected_output"
            && frame["views"]["status"]["data"]["hosts"][1]["sampled_ms"] == sampled
    });
    let (_, status) = json_read(&["status"]);
    assert_eq!(status["hosts"].as_array().unwrap().len(), 2);
    assert!(
        status["hosts"][1]["error"]
            .as_str()
            .unwrap()
            .contains("unexpected output")
    );
    assert_observer_gap_diagnostics(observer);
}

fn assert_observer_gap_diagnostics(mut observer: Background) {
    observer.owned.terminate();
    let (_, stderr) = observer.output();
    let mut counts = std::collections::BTreeMap::new();
    for line in stderr.lines().filter(|line| line.contains("collection ")) {
        *counts.entry(line).or_insert(0_usize) += 1;
    }
    for domain in ["agents", "headroom", "host", "status"] {
        let prefix = format!("dogsled service: {domain} collection failed:");
        assert_eq!(
            stderr
                .lines()
                .filter(|line| line.starts_with(&prefix))
                .count(),
            1,
            "{stderr}"
        );
    }
    assert!(
        stderr.contains(
            "peer:8103 collection failure changed to unavailable: unreachable: unexpected output"
        ),
        "{stderr}"
    );
    assert_eq!(stderr.lines().filter(|line| line == &"dogsled service: peer:8103:branches collection failed: branch observation requires fresh execution-host facts").count(), 2, "{stderr}");
    eprintln!("observer failure line counts: {counts:?}");
}

fn wait_publication(rig: &Rig, ready: impl Fn(&Value) -> bool) -> Value {
    wait_publication_at(rig, &rig.dir.join("state"), ready)
}

fn wait_publication_at(rig: &Rig, state: &Path, ready: impl Fn(&Value) -> bool) -> Value {
    let deadline = Instant::now() + PROCESS_DEADLINE;
    let mut last = String::new();
    loop {
        if std::os::unix::net::SocketAddr::from_pathname(state.join("observation.sock")).is_err() {
            // The production client owns secure fallback-path resolution for long states.
            let output = rig
                .cmd(&["observation", "current"])
                .env("DOGSLED_STATE", state)
                .output()
                .unwrap();
            if output.status.success()
                && let Ok(frame) = serde_json::from_slice::<Value>(&output.stdout)
                && ready(&frame)
            {
                return frame;
            }
            last = String::from_utf8_lossy(&output.stderr).into_owned();
        }
        if let Ok(mut socket) =
            std::os::unix::net::UnixStream::connect(state.join("observation.sock"))
        {
            use std::io::{BufRead as _, Write as _};
            socket
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            socket
                .write_all(b"{\"version\":1,\"operation\":\"current\"}\n")
                .unwrap();
            let mut line = String::new();
            if std::io::BufReader::new(socket).read_line(&mut line).is_ok()
                && let Ok(frame) = serde_json::from_str::<Value>(&line)
                && ready(&frame)
            {
                return frame;
            }
            last = line;
        }
        assert!(
            Instant::now() < deadline,
            "observer did not publish expected facts at {}: {last}; herdr: {}",
            rig.dir.display(),
            rig.log("herdr.log")
        );
        std::thread::yield_now();
    }
}

#[test]
fn retry_without_serve_resets_backoff_but_never_starts_ssh() {
    let rig = Rig::new("retry-no-observer");
    rig.peer();
    rig.ingest_peers();
    let reads = rig.log("ssh.log");
    let (code, error) = rig.json(&["retry", "netcup"]);
    assert_eq!(code, 2);
    assert!(error["error"].as_str().unwrap().contains("backoff reset"));
    assert!(
        error["error"]
            .as_str()
            .unwrap()
            .contains("serve is not running")
    );
    assert_eq!(rig.log("ssh.log"), reads);
    assert_eq!(rig.state(&["peer", "8103"])["fetched_ms"], 0);
}

#[test]
fn brief_keeps_every_host_within_six_lines_and_line_sanitizes_labels() {
    let rig = Rig::new("large-fleet");
    rig.snapshot("remote.json", now(), 0.0, 4);
    rig.put(
        "machines.json",
        &json!((0..8).map(|i| json!({
        "id":format!("h{i}"), "label":format!("peer-{i}\nlabel"), "target":format!("h{i}")
    })).collect::<Vec<_>>()),
    );
    let _observer = rig.serve();
    wait_publication(&rig, |frame| {
        frame["views"]["status"]["data"]["hosts"]
            .as_array()
            .is_some_and(|hosts| hosts.len() == 9)
    });
    let brief = String::from_utf8(rig.run(&["status", "--brief"]).stdout).unwrap();
    assert_eq!(brief.lines().count(), 6);
    for i in 0..8 {
        assert!(brief.contains(&format!("peer-{i} label")));
    }
    let line = String::from_utf8(rig.run(&["status", "--line"]).stdout).unwrap();
    assert_eq!(line.lines().count(), 1);
}

#[test]
fn concurrent_status_callers_share_one_peer_cache_fill() {
    let rig = Rig::new("concurrent-cache");
    rig.peer();
    let _observer = rig.serve();
    wait_publication(&rig, |frame| {
        frame["views"]["status"]["data"]["hosts"][1]["sampled_ms"].is_u64()
    });
    let reads = rig.log("ssh.log").lines().count();
    assert_eq!(rig.log("ssh.log").matches("host snapshot").count(), 1);
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..8)
            .map(|_| scope.spawn(|| rig.json(&["status"]).0))
            .collect();
        for handle in handles {
            assert_eq!(handle.join().unwrap(), 0);
        }
    });
    assert_eq!(
        rig.log("ssh.log").lines().count(),
        reads,
        "concurrent readers do no SSH"
    );
}

#[test]
fn peers_are_read_in_parallel() {
    let rig = Rig::new("parallel");
    rig.put(
        "machines.json",
        &json!([
            {"id":"a","label":"a","target":"a"}, {"id":"b","label":"b","target":"b"}
        ]),
    );
    rig.snapshot("remote.json", now(), 0.0, 4);
    // FIFOs form a rendezvous. Sequential peer reads deadlock; no sleeps or
    // timing assertions are involved. The test coordinator bounds the child.
    for name in ["a", "b"] {
        assert!(
            Command::new("mkfifo")
                .arg(rig.dir.join(name))
                .status()
                .unwrap()
                .success()
        );
    }
    rig.script(
        "ssh",
        r#"#!/bin/sh
case "$*" in
 *'-- a '*) printf ready > "$RIG/a"; read -r ready < "$RIG/b" ;;
 *'-- b '*) read -r ready < "$RIG/a"; printf ready > "$RIG/b" ;;
esac
cat "$RIG/remote.json"
"#,
    );
    // The test owns the command as its own child, so the test reaps it and reads its
    // exit status. The guard kills the command's group, so the peers it started never
    // outlive the test.
    let mut command = rig.cmd(&["serve", "--once", "--interval", "1"]);
    command.stdout(std::process::Stdio::piped());
    let mut owned = Owned::spawn(&mut command);
    let status = owned.wait();
    assert!(status.success(), "the peer rendezvous failed");
}

#[test]
fn place_reserves_most_headroom_and_respects_host_pin() {
    let rig = Rig::new("place");
    rig.peer();
    rig.ingest_peers();
    rig.snapshot("state", now(), 0.0, 2);
    let _publisher = observation::seeded(&rig);
    let (code, placed) = rig.json(&["place"]);
    assert_eq!(code, 0, "{placed}");
    assert_eq!(placed["host"]["id"], "8103");
    assert!(rig.log("ssh.log").contains("host reserve"));
    let (code, placed) = rig.json(&["place", "--host", "local"]);
    assert_eq!(code, 0, "{placed}");
    assert_eq!(placed["host"]["id"], "local");
    let reservations = rig.reservations();
    assert_eq!(reservations.as_array().unwrap().len(), 1);
    assert_eq!(reservations[0]["id"], placed["reservation"]);
    assert_eq!(rig.json(&["place", "--host", "missing"]).0, 2);
}

#[test]
fn place_falls_back_when_remote_capacity_changed_and_refuses_low_disk() {
    let rig = Rig::new("fallback");
    rig.peer();
    rig.ingest_peers();
    rig.snapshot("state", now(), 0.0, 2);
    rig.put("reserve.json", &json!({"reserved":false,"headroom":null}));
    let observer = observation::seeded(&rig);
    let (code, placed) = rig.json(&["place"]);
    drop(observer);
    assert_eq!(code, 0, "{placed}");
    assert_eq!(placed["host"]["id"], "local");
    rig.script("df", "#!/bin/sh\nprintf 'Filesystem 1024-blocks Used Available Capacity Mounted on\nfixture 1000 860 140 86%% /\n'\n");
    let _observer = rig.serve_at(&rig.dir.join("state"), "3600");
    wait_publication(&rig, |frame| {
        frame["local"]["host"]["probe"]["disk_free_percent"]
            .as_f64()
            .is_some_and(|free| (free - 14.0).abs() < 0.001)
    });
    assert_eq!(rig.json(&["place", "--host", "local"]).0, 1);
    let status = rig.run(&["status"]);
    let text = String::from_utf8(status.stdout).unwrap();
    assert!(
        text.contains("disk 14.0% free"),
        "status={text}; publication={}",
        rig.json(&["observation", "current"]).1
    );
}

#[test]
fn full_is_exit_one_with_durable_queue() {
    let rig = Rig::new("full");
    rig.snapshot("state", now(), 3.0, 2);
    let publisher = observation::seeded(&rig);
    let output = rig.run(&["place"]);
    assert_eq!(output.status.code(), Some(1));
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["queued"], true);
    assert!(value["ticket"].is_string());
    assert_eq!(rig.tickets()[0]["state"], "pending");
    drop(publisher);
    rig.snapshot("state", 0, 0.0, 2);
    let _publisher = observation::seeded(&rig);
    assert_eq!(rig.json(&["place"]).0, 1);
}

#[test]
fn concurrent_placement_rechecks_capacity_under_host_lock() {
    let rig = Rig::new("concurrent-place");
    rig.snapshot("state", now(), 2.0, 2);
    let _publisher = observation::seeded(&rig);
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..8)
            .map(|_| scope.spawn(|| rig.json(&["place"]).0))
            .collect();
        let codes: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        assert_eq!(codes.iter().filter(|&&code| code == 0).count(), 1);
        assert_eq!(codes.iter().filter(|&&code| code == 1).count(), 7);
    });
    assert_eq!(rig.reservations().as_array().unwrap().len(), 1);
}

#[test]
fn reservation_survives_agent_discovery_and_ends_only_at_ttl() {
    let rig = Rig::new("reservation");
    rig.snapshot("state", now(), 2.0, 2);
    let publisher = observation::seeded(&rig);
    assert_eq!(rig.json(&["place"]).0, 0);
    assert!(
        !rig.log("herdr-roles.log")
            .lines()
            .any(|line| line == "actor agent list")
    );
    rig.put(
        "agents.json",
        &json!({"result":{"agents":[{"pane_id":"w1:p8"}]}}),
    );
    assert_eq!(rig.json(&["place"]).0, 1);
    let queued = rig.tickets()[0]["id"].as_str().unwrap().to_owned();
    assert!(rig.run(&["cancel", &queued]).status.success());
    drop(publisher);
    assert!(rig.run(&["serve", "--once"]).status.success());
    assert_eq!(rig.reservations().as_array().unwrap().len(), 1);
    rig.put_reservations(&json!([{"id":"expired","created_ms":now()-90_001}]));
    assert!(rig.run(&["serve", "--once"]).status.success());
    assert_eq!(rig.reservations(), json!([]));
    for command in ["bind", "release"] {
        assert_eq!(rig.run(&["host", command]).status.code(), Some(2));
    }
}

/// A unit admission for `w1:p1` whose origin saved a reference release that the host has
/// not acknowledged.
pub(crate) fn released_admission(id: &str, origin: &str) -> Value {
    let execution = json!({"executor":"dogsled","ownerId":"w1:p1","requestId":id});
    let unit_ref =
        json!({"sessionId":"session-1","unitId":"u-1","revision":1,"contractEntryId":"entry-1"});
    let digest = "0".repeat(64);
    let mut request = place_request(id);
    request["owner_host"] = json!(origin);
    json!({"request":request,"launch":null,"error":null,"result":{},"unit":{
        "origin":unit_ref,"execution":execution,"placement":{},"tool_call_id":"call-1",
        "committed":null,"abandoned":null,"reference_release":{"certificate":{
            "version":2,"origin_host":origin,"execution":execution,
            "binding":{"contract":unit_ref,"execution":execution,"exportDigest":digest},
            "launch":{"id":id,"action":{"type":"place"},"host":"worker","cwd":"/","pane":"w2:p1",
                "workspace":null,"workspace_name":null,"owner":"w1:p1","owner_host":origin},
            "terminal_evidence":{"ack":{"execution":execution,"unitRef":unit_ref,"digest":digest,
                "sealEntryId":"seal","sessionLeafId":"leaf"},"sessionId":"session-1",
                "replyEntryId":"reply","text":"FINAL"},
            "custody_revision":0,"released_references":["execution"]},
            "acknowledgement":null,"reason":null}}})
}

/// A minimal valid request to place work for `w1:p1`.
pub(crate) fn place_request(id: &str) -> Value {
    json!({"id":id,"action":{"type":"place"},"host":null,"branch":null,"base":null,"cwd":"/",
        "repository":null,"workspace":null,"workspace_name":null,"owner":"w1:p1"})
}

fn repository(path: &Path) {
    fs::create_dir_all(path).unwrap();
    assert!(
        isolated("git")
            .args(["init", "-q", "-b", "main"])
            .arg(path)
            .status()
            .unwrap()
            .success()
    );
    assert!(
        isolated("git")
            .arg("-C")
            .arg(path)
            .args([
                "remote",
                "add",
                "origin",
                "https://example.invalid/repo.git"
            ])
            .status()
            .unwrap()
            .success()
    );
}

#[test]
fn remote_prepare_uses_remote_command_and_home_relative_checkout() {
    let rig = Rig::new("remote-prepare");
    let repo = rig.dir.join("home/repo space");
    repository(&repo);
    rig.peer();
    rig.config("remote_command = ['custom', 'dogsled']\n");
    rig.ingest_peers();
    let observer = place::peer_observer(&rig);
    let (code, placed) = rig.json(&[
        "place",
        "--host",
        "netcup",
        "--branch",
        "feature",
        "--base",
        "main",
        "--repo",
        repo.to_str().unwrap(),
    ]);
    drop(observer);
    assert_eq!(code, 0, "{placed}");
    assert_eq!(placed["path"], "/remote/tree");
    let ssh = rig.log("ssh.log");
    let prepares: Vec<_> = ssh
        .lines()
        .filter(|line| line.contains("host prepare "))
        .collect();
    assert_eq!(prepares.len(), 1, "{ssh}");
    assert_eq!(prepares[0].matches("--request-id ").count(), 1, "{ssh}");
    let id = prepares[0]
        .split_once("--request-id ")
        .unwrap()
        .1
        .split_whitespace()
        .next()
        .unwrap();
    let parts: Vec<_> = id.split('-').collect();
    assert_eq!(parts.len(), 4, "{id}");
    assert_eq!(parts[0], "r", "{id}");
    assert!(
        parts[1..]
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit())),
        "{id}"
    );
    let admission = rig.admission(id);
    let grant = &admission["placement"]["saved"]["prepared"]["grant"];
    let mut expected_replay = grant["envelope"].clone();
    expected_replay["expected_grant"] = grant.clone();
    let replay = prepares[0]
        .split_once("--placement-replay '")
        .unwrap()
        .1
        .split_once("' --budget-ms")
        .unwrap()
        .0;
    // `shell::quote` writes an embedded `'` as `'\''`, for example in a host name.
    assert_eq!(
        serde_json::from_str::<Value>(&replay.replace("'\\''", "'")).unwrap(),
        expected_replay
    );
    let created = grant["envelope"]["request"]["validity"]["created_ms"]
        .as_u64()
        .unwrap();
    assert!(prepares[0].ends_with(&format!("custom dogsled host prepare --preparation-envelope-version 2 --request-created-ms {created} --preparation-protocol 1 --repo 'repo space' --branch feature --request-id {id} --base main --placement-replay '{replay}' --budget-ms 600000 --json")), "{ssh}");
    let releases: Vec<_> = ssh
        .lines()
        .filter(|line| line.contains("host prepared "))
        .collect();
    assert_eq!(releases.len(), 1, "{ssh}");
    assert!(
        releases[0].contains("custom dogsled host prepared --preparation-protocol 1 --input "),
        "{ssh}"
    );
    assert!(
        releases[0].contains(&format!("\"request_id\":\"{id}\"")),
        "{ssh}"
    );
    assert!(releases[0].contains("/remote/tree"), "{ssh}");
    assert!(releases[0].ends_with("--budget-ms 600000 --json"), "{ssh}");
    assert!(!ssh.contains("host bind"));
}

#[test]
fn removed_commands_and_sidebar_are_not_accepted() {
    let rig = Rig::new("removed");
    for cmd in [
        "spawn", "admit", "lead", "mark", "queue", "cancel", "release", "report", "probe",
        "prepare", "notify",
    ] {
        assert_eq!(rig.run(&[cmd]).status.code(), Some(2), "{cmd}");
    }
    assert_eq!(rig.run(&["status", "--sidebar"]).status.code(), Some(2));
    assert_eq!(
        rig.run(&["status", "--line", "--brief"]).status.code(),
        Some(2)
    );
}

#[test]
fn watch_prints_a_frame_without_waiting_for_the_interval() {
    let rig = Rig::new("watch");
    rig.snapshot("state", now(), 1.0, 2);
    let mut frame = observation::publication(1);
    frame["views"]["status_display"]["data"] =
        observation::display(vec![observation::host_display(
            "local",
            "1/2",
            "eligible",
            Some(now()),
            "service: up",
        )]);
    let _publisher = observation::Supplied::start(&rig, frame);
    // The test owns the command as its own child, so the test reaps it. The guard kills
    // the command's group, so the watch never outlives the test.
    let mut command = rig.cmd(&["status", "--watch", "--line", "--interval", "3600"]);
    command.stdout(std::process::Stdio::piped());
    let mut owned = Owned::spawn(&mut command);
    let stdout = owned.child_mut().stdout.take().unwrap();
    let (send, recv) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        use std::io::BufRead as _;
        send.send(
            std::io::BufReader::new(stdout)
                .lines()
                .next()
                .unwrap()
                .unwrap(),
        )
        .unwrap();
    });
    let frame = recv.recv_timeout(std::time::Duration::from_secs(10));
    owned.child_mut().kill().unwrap();
    reader.join().unwrap();
    assert_eq!(frame.unwrap(), "local 1/2");
}

/// Dropping a background command kills the command and its group before the guard joins
/// the reader. The reader ends only when the command's streams close, so a drop that
/// joins the reader first waits on a command that never ends. The drop runs on its own
/// thread, so a drop that waits fails the test on the bound instead of hanging it.
#[test]
fn draining_background_stderr_does_not_wait_for_stdout_eof() {
    let rig = Rig::new("background-stderr");
    let ready = rig.dir.join("stderr-drained");
    let mut command = Command::new("sh");
    command
        .args([
            "-c",
            "dd if=/dev/zero bs=65536 count=32 >&2 2>/dev/null; printf ready > \"$1\"; sleep 60",
            "fixture",
        ])
        .arg(&ready);
    let mut background = Rig::background(command);
    let deadline = Instant::now() + Duration::from_secs(5);
    while !ready.exists() {
        assert!(Instant::now() < deadline, "stderr pipe blocked the child");
        std::thread::yield_now();
    }
    assert!(background.owned.child_mut().try_wait().unwrap().is_none());
}

#[test]
fn dropping_a_background_command_kills_it_before_the_guard_joins_its_reader() {
    let rig = Rig::new("background-drop");
    rig.snapshot("state", now(), 1.0, 2);
    let _observer = rig.serve_at(&rig.dir.join("state"), "3600");
    // The command never ends on its own: it watches until the test ends it.
    let background = rig.start(&["status", "--watch", "--line", "--interval", "3600"]);
    let pid = background.owned.pid;
    let killer = background.owned.group_killer();
    let (send, recv) = std::sync::mpsc::channel();
    let dropper = std::thread::spawn(move || {
        drop(background);
        let _ = send.send(());
    });
    if recv.recv_timeout(Duration::from_secs(10)).is_err() {
        // The drop is still waiting on the reader. The test kills the command's group,
        // so no process outlives the test and the drop ends.
        killer.kill();
        let _ = dropper.join();
        panic!("the drop joined the reader of a command that never ends");
    }
    dropper.join().unwrap();
    assert!(
        wait_gone(pid, PROCESS_DEADLINE),
        "the command outlived its drop"
    );
}

/// Typed state inspection through the binary, including the peer host fixture.
struct PreparationRecord<'a> {
    rig: &'a Rig,
    state: &'a str,
    kind: &'a str,
    id: String,
}
impl PreparationRecord<'_> {
    fn args(&self) -> Vec<&str> {
        if self.kind == "claims" {
            vec!["state", "preparations"]
        } else {
            vec!["state", "preparation", self.kind, &self.id]
        }
    }
    fn read(&self) -> Value {
        let out = self
            .rig
            .cmd(&self.args())
            .env("DOGSLED_STATE", self.rig.dir.join(self.state))
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        serde_json::from_slice(&out.stdout).unwrap()
    }
    fn exists(&self) -> bool {
        if self.kind == "claims" {
            !self.read().as_array().unwrap().is_empty()
        } else {
            !self.read().is_null()
        }
    }
    fn put(&self, value: &Value) {
        let mut args = self.args();
        args.push("--put");
        self.rig.put_state_in(&args, value, self.state);
    }
    fn delete(&self) {
        let mut args = self.args();
        args.push("--delete");
        let out = self
            .rig
            .cmd(&args)
            .env("DOGSLED_STATE", self.rig.dir.join(self.state))
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
    }
}
