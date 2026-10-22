//! Exercise the real commands with private state and fake Herdr/SSH on PATH.
#![allow(clippy::unwrap_used, clippy::panic)]

#[path = "cli/close.rs"]
mod close;
#[path = "cli/context.rs"]
mod context;
#[path = "cli/done.rs"]
mod done;
#[path = "cli/inbox.rs"]
mod inbox;
#[path = "cli/jobs.rs"]
mod jobs;
#[path = "cli/launch_retry.rs"]
mod launch_retry;
#[path = "cli/legacy_continue.rs"]
mod legacy_continue;
#[path = "cli/lifecycle.rs"]
mod lifecycle;
#[path = "cli/machines.rs"]
mod machines;
#[path = "cli/mcp.rs"]
mod mcp;
#[path = "cli/place.rs"]
mod place;
#[path = "cli/placement.rs"]
mod placement;
#[path = "cli/profiles.rs"]
mod profiles;
#[path = "cli/review.rs"]
mod review;
#[path = "cli/review_fixes.rs"]
mod review_fixes;
#[path = "cli/stop_sweep.rs"]
mod stop_sweep;
#[path = "cli/sweep.rs"]
mod sweep;
#[path = "cli/transcripts.rs"]
mod transcripts;
#[path = "cli/unit_requests.rs"]
mod unit_requests;
#[path = "cli/unit_seeded.rs"]
mod unit_seeded;
#[path = "cli/watchdog.rs"]
mod watchdog;

use serde_json::{Value, json};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

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
const CONTRACT: &str = "Dogsled started you as a child agent. When your whole task is complete, call the dogsled `done` tool (or run `dogsled done`) once, then end your turn with your final reply. Do not call `done` when you ask a question, wait for input, or own open children. Commit your changes first. `done` hands your work back: Dogsled closes your pane and agent worktree when this turn ends, keeps your branch, and leaves landing to your owner. After that turn, messages cannot reopen you; more work needs a new child.";

struct Rig {
    dir: PathBuf,
    service: std::sync::Mutex<Option<fs::File>>,
}
impl Rig {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("dogsled-cli-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        for child in ["bin", "state", "home"] {
            fs::create_dir_all(dir.join(child)).unwrap();
        }
        let rig = Self {
            dir,
            service: std::sync::Mutex::new(None),
        };
        rig.put("machines.json", &json!([]));
        rig.put("agents.json", &json!({"result":{"agents":[]}}));
        rig.put(
            "panes.json",
            &json!({"result":{"panes":[{"pane_id":"w1:p1","terminal_id":"term_owner"},{"pane_id":"w2:p1","terminal_id":"term_child"}]}}),
        );
        rig.script(
            "herdr",
            r#"#!/bin/sh
printf '%s\n' "$*" >> "$RIG/herdr.log"
# The line Herdr types for `agent start`: the kind, then each argument shell-quoted.
# A terminal refuses a line over 1024 bytes, so a longer one fails like the real thing.
if test "$1 $2" = 'agent start'; then
  (
    kind=$5; shift 8
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
  'pane process-info'*) printf '%s\n' '{"result":{"process_info":{"shell_pid":10,"foreground_processes":[{"pid":10,"name":"zsh"}]}}}' ;;
  'worktree list --cwd'*) cat "$RIG/worktrees.json" ;;
  'tab create'*|'workspace create'*) printf '%s\n' '{"result":{"root_pane":{"pane_id":"w2:p1","terminal_id":"term_child"}}}' ;;
  'pane run'*) shift 3; sh -c "$*" > "$RIG/job-output" 2>&1; printf '{}\n' ;;
  'pane close '*)
    mkdir -p "$RIG/at-close"
    cp "$DOGSLED_STATE"/jobs/*.json "$RIG/at-close/"
    if test -f "$RIG/fail-close"; then echo close-refused >&2; exit 1; fi
    printf '{}\n' ;;
  'agent start'*|'agent prompt'*|'pane report-agent'*|'pane report-metadata'*) printf '{}\n' ;;
  'machine rename'*|'machine enable'*|'machine disable'*) printf '{}\n' ;;
  *) echo unexpected >&2; exit 2 ;;
esac
"#,
        );
        rig.script(
            "ssh",
            r#"#!/bin/sh
printf '%s\n' "$*" >> "$RIG/ssh.log"
case "$*" in
  *'host snapshot'*)
    if test -f "$RIG/unreachable"; then echo offline >&2; exit 255; fi
    cat "$RIG/remote.json" ;;
  *'host reserve'*) cat "$RIG/reserve.json" ;;
  *'host checkout'*) printf '%s\n' '"/remote/repo"' ;;
  *'host launch'*) printf '%s\n' '{"id":"remote-job","host":"8103","pane":"w9:p1","name":"worker"}' ;;
  *'host jobs'*) cat "$RIG/jobs.json" ;;
  *'host job '*) cat "$RIG/job.json" ;;
  *'host logs'*) cat "$RIG/logs.json" ;;
  *'host stop'*) printf '{}\n' ;;
  *'host prepare'*) printf '%s\n' '{"pane_id":"w9:p1","terminal_id":"term_remote","path":"/remote/tree"}' ;;
  *) echo unexpected >&2; exit 2 ;;
esac
"#,
        );
        rig.put("reserve.json", &json!({"reserved":true,"headroom":null}));
        rig
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
            *service = Some(lock);
        }
    }
    fn put(&self, path: &str, value: &Value) {
        if path == "state/host.json" {
            self.service();
        }
        fs::write(
            self.dir.join(path),
            serde_json::to_vec_pretty(value).unwrap(),
        )
        .unwrap();
    }
    fn script(&self, name: &str, text: &str) {
        let path = self.dir.join("bin").join(name);
        fs::write(&path, text).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    fn config(&self, text: &str) {
        fs::write(self.dir.join("config.toml"), text).unwrap();
    }
    fn cmd(&self, args: &[&str]) -> Command {
        if args.first() == Some(&"serve") {
            self.service.lock().unwrap().take();
        }
        let mut cmd = isolated(env!("CARGO_BIN_EXE_dogsled"));
        cmd.args(args)
            .env(
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
            .env("RIG", &self.dir)
            .env("HOME", self.dir.join("home"))
            .env("DOGSLED_CONFIG", self.dir.join("config.toml"))
            .env("DOGSLED_STATE", self.dir.join("state"));
        if let Ok(now) = fs::read_to_string(self.dir.join("clock")) {
            cmd.env("DOGSLED_TEST_NOW_MS", now.trim());
        }
        if let Ok(window) = fs::read_to_string(self.dir.join("busy-window-ms")) {
            cmd.env("DOGSLED_TEST_BUSY_WINDOW_MS", window.trim());
        }
        cmd
    }
    fn run(&self, args: &[&str]) -> Output {
        self.cmd(args).output().unwrap()
    }
    fn json(&self, args: &[&str]) -> (i32, Value) {
        let out = self.cmd(args).arg("--json").output().unwrap();
        let value = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
            panic!(
                "{e}: {} {}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            )
        });
        (out.status.code().unwrap(), value)
    }
    fn snapshot(&self, path: &str, sampled: u64, load: f64, cores: u32) {
        // Busy fraction that reproduces the old load rule's verdict for this load. The zero
        // tick counters let a Linux `serve --once` derive a busy sample from one real reading.
        let busy = (load / (1.5 * f64::from(cores))).min(1.0);
        self.put(path, &json!({"sampled_ms":sampled,"probe":{
            "host":"local","label":"local","load1":load,"cores":cores,"busy":busy,
            "cpu_ticks":{"busy":0,"total":0},
            "memory":{"used_bytes":1_073_741_824_u64,"total_bytes":8_589_934_592_u64},
            "disk_free_percent":50.0,"agents":{"total":1,"by_status":{"idle":1}},
            "agent_panes":[{"pane":"w1:p1","agent":"pi","state":"idle","name":null,"transcript":null,"transcript_modified":null}],"reservations":[],"panes":[]
        }}));
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
        self.put(
            "state/done.json",
            &json!({terminal: {"pane":pane,"after":seq - 1,"sealed":seq,"marked_ms":1}}),
        );
    }
    fn read(&self, path: &str) -> Value {
        serde_json::from_slice(&fs::read(self.dir.join(path)).unwrap()).unwrap()
    }
    fn log(&self, path: &str) -> String {
        fs::read_to_string(self.dir.join(path)).unwrap_or_default()
    }
    fn expire_cache(&self) {
        let mut cache = self.read("state/peer-38313033.json");
        cache["fetched_ms"] = json!(0);
        cache["retry"] = json!({});
        self.put("state/peer-38313033.json", &cache);
    }
}
impl Drop for Rig {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn serve_once_publishes_memory_agents_and_all_panes_without_ssh_or_discovery() {
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
    let snapshot = rig.read("state/host.json");
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
    rig.snapshot("state/host.json", 1, 2.0, 4);
    let before = fs::read(rig.dir.join("state/host.json")).unwrap();
    rig.script("herdr", "#!/bin/sh\necho disconnected >&2\nexit 1\n");
    assert_eq!(rig.run(&["serve", "--once"]).status.code(), Some(2));
    assert_eq!(fs::read(rig.dir.join("state/host.json")).unwrap(), before);
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
    let snapshot = rig.read("state/host.json");
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
    rig.snapshot("state/host.json", 1000, 0.0, 8);
    let status = rig.json(&["status"]).1;
    assert_eq!(status["hosts"][0]["service"], "up");
    assert_eq!(status["hosts"][0]["sampled_ms"], 1000);
    assert_eq!(status["hosts"][0]["headroom"]["eligible"], false);
    let text = rig.run(&["status", "--brief"]);
    assert!(String::from_utf8_lossy(&text.stdout).contains("service: up (snapshot 1m ago)"));
    let live = rig.json(&["host", "snapshot"]).1;
    assert_eq!(live["running"], true);
    assert_eq!(live["sampled_ms"], 1000);
    rig.service.lock().unwrap().take();
    assert_eq!(rig.json(&["status"]).1["hosts"][0]["service"], "down");
    assert_eq!(rig.json(&["host", "snapshot"]).1["running"], false);
}

#[test]
fn status_reads_snapshots_and_caches_peer_reads() {
    let rig = Rig::new("status-cache");
    rig.peer();
    rig.snapshot("state/host.json", now(), 50.0, 12);
    rig.config("remote_command = ['custom-dogsled', 'argument with spaces']\n");
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
    assert_eq!(rig.log("ssh.log").lines().count(), 1);
    assert!(
        rig.log("ssh.log")
            .contains("custom-dogsled 'argument with spaces' host snapshot --json")
    );
    assert!(!rig.log("herdr.log").contains("agent list"));
    rig.expire_cache();
    rig.json(&["status"]);
    assert_eq!(rig.log("ssh.log").lines().count(), 2);
}

#[test]
fn missing_stale_unreachable_and_malformed_hosts_remain_visible() {
    let rig = Rig::new("down");
    rig.peer();
    rig.snapshot("remote.json", now() - 31_000, 6.0, 7);
    let (_, status) = rig.json(&["status"]);
    assert_eq!(status["hosts"][0]["service"], "down");
    assert!(status["hosts"][0]["probe"].is_null());
    assert_eq!(status["hosts"][1]["service"], "down");
    assert_eq!(status["hosts"][1]["probe"]["load1"], 6.0);
    assert_eq!(status["hosts"][1]["headroom"]["eligible"], false);
    assert_eq!(
        String::from_utf8(rig.run(&["status", "--line"]).stdout).unwrap(),
        "local ? · netcup 4/7 ?\n"
    );
    rig.expire_cache();
    fs::write(rig.dir.join("unreachable"), "").unwrap();
    let (_, status) = rig.json(&["status"]);
    assert_eq!(status["hosts"][1]["reachable"], false);
    assert_eq!(status["hosts"][1]["probe"]["load1"], 6.0);
    // The panel shows when the next read happens; the board carries the same time.
    let retry = status["hosts"][1]["retry_ms"].as_u64().unwrap();
    assert!(retry > now(), "{retry}");
    let (_, board) = rig.json(&["board", "--all"]);
    assert_eq!(board["hosts"][1]["retry_ms"], retry);
    assert!(
        String::from_utf8(rig.run(&["status", "--line"]).stdout)
            .unwrap()
            .contains("netcup 4/7 ?")
    );
    // "Retry now" reads the peer at once instead of waiting out the backoff.
    fs::remove_file(rig.dir.join("unreachable")).unwrap();
    let reads = rig.log("ssh.log").lines().count();
    let (code, report) = rig.json(&["retry", "netcup"]);
    assert_eq!(code, 0, "{report}");
    assert_eq!(report["reachable"], true);
    assert!(report["retry_ms"].is_null());
    assert_eq!(rig.log("ssh.log").lines().count(), reads + 1);
    rig.expire_cache();
    rig.put("remote.json", &Value::Null);
    let (_, status) = rig.json(&["status"]);
    assert_eq!(status["hosts"][1]["service"], "down");
    assert_eq!(status["hosts"][1]["probe"]["load1"], 6.0);
    rig.expire_cache();
    fs::write(rig.dir.join("remote.json"), "bad json").unwrap();
    let (_, status) = rig.json(&["status"]);
    assert_eq!(status["hosts"].as_array().unwrap().len(), 2);
    assert!(
        status["hosts"][1]["error"]
            .as_str()
            .unwrap()
            .contains("unexpected output")
    );
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
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..8)
            .map(|_| scope.spawn(|| rig.json(&["status"]).0))
            .collect();
        for handle in handles {
            assert_eq!(handle.join().unwrap(), 0);
        }
    });
    assert_eq!(rig.log("ssh.log").lines().count(), 1);
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
    let mut child = rig
        .cmd(&["status", "--json"])
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let (send, recv) = std::sync::mpsc::channel();
    let pid = child.id();
    let waiter = std::thread::spawn(move || {
        send.send(child.wait().unwrap()).unwrap();
    });
    match recv.recv_timeout(std::time::Duration::from_secs(10)) {
        Ok(status) => assert!(status.success()),
        Err(error) => {
            let _ = Command::new("kill")
                .args(["-KILL", &pid.to_string()])
                .status();
            panic!("peer rendezvous failed: {error}");
        }
    }
    waiter.join().unwrap();
}

#[test]
fn place_reserves_most_headroom_and_respects_host_pin() {
    let rig = Rig::new("place");
    rig.peer();
    rig.snapshot("state/host.json", now(), 0.0, 2);
    let (code, placed) = rig.json(&["place"]);
    assert_eq!(code, 0, "{placed}");
    assert_eq!(placed["host"]["id"], "8103");
    assert!(rig.log("ssh.log").contains("host reserve"));
    let (code, placed) = rig.json(&["place", "--host", "local"]);
    assert_eq!(code, 0, "{placed}");
    assert_eq!(placed["host"]["id"], "local");
    let reservations = rig.read("state/placements.json");
    assert_eq!(reservations.as_array().unwrap().len(), 1);
    assert_eq!(reservations[0]["id"], placed["reservation"]);
    assert_eq!(rig.json(&["place", "--host", "missing"]).0, 2);
}

#[test]
fn place_falls_back_when_remote_capacity_changed_and_refuses_low_disk() {
    let rig = Rig::new("fallback");
    rig.peer();
    rig.snapshot("state/host.json", now(), 0.0, 2);
    rig.put("reserve.json", &json!({"reserved":false,"headroom":null}));
    let (code, placed) = rig.json(&["place"]);
    assert_eq!(code, 0, "{placed}");
    assert_eq!(placed["host"]["id"], "local");
    let mut snapshot = rig.read("state/host.json");
    snapshot["probe"]["disk_free_percent"] = json!(14.0);
    rig.put("state/host.json", &snapshot);
    assert_eq!(rig.json(&["place", "--host", "local"]).0, 1);
    assert!(
        String::from_utf8(rig.run(&["status"]).stdout)
            .unwrap()
            .contains("disk 14.0% free")
    );
}

#[test]
fn full_is_exit_one_with_durable_queue() {
    let rig = Rig::new("full");
    rig.snapshot("state/host.json", now(), 3.0, 2);
    let output = rig.run(&["place"]);
    assert_eq!(output.status.code(), Some(1));
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["queued"], true);
    assert!(value["ticket"].is_string());
    assert_eq!(rig.read("state/queue.json")[0]["state"], "pending");
    rig.snapshot("state/host.json", 0, 0.0, 2);
    assert_eq!(rig.json(&["place"]).0, 1);
}

#[test]
fn concurrent_placement_rechecks_capacity_under_host_lock() {
    let rig = Rig::new("concurrent-place");
    rig.snapshot("state/host.json", now(), 2.0, 2);
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..8)
            .map(|_| scope.spawn(|| rig.json(&["place"]).0))
            .collect();
        let codes: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        assert_eq!(codes.iter().filter(|&&code| code == 0).count(), 1);
        assert_eq!(codes.iter().filter(|&&code| code == 1).count(), 7);
    });
    assert_eq!(
        rig.read("state/placements.json").as_array().unwrap().len(),
        1
    );
}

#[test]
fn reservation_survives_agent_discovery_and_ends_only_at_ttl() {
    let rig = Rig::new("reservation");
    rig.snapshot("state/host.json", now(), 2.0, 2);
    assert_eq!(rig.json(&["place"]).0, 0);
    assert!(!rig.log("herdr.log").contains("agent list"));
    rig.put(
        "agents.json",
        &json!({"result":{"agents":[{"pane_id":"w1:p8"}]}}),
    );
    assert_eq!(rig.json(&["place"]).0, 1);
    let queued = rig.read("state/queue.json")[0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(rig.run(&["cancel", &queued]).status.success());
    assert!(rig.run(&["serve", "--once"]).status.success());
    assert_eq!(
        rig.read("state/placements.json").as_array().unwrap().len(),
        1
    );
    rig.put(
        "state/placements.json",
        &json!([{"id":"expired","created_ms":now()-90_001}]),
    );
    assert!(rig.run(&["serve", "--once"]).status.success());
    assert_eq!(rig.read("state/placements.json"), json!([]));
    for command in ["bind", "release"] {
        assert_eq!(rig.run(&["host", command]).status.code(), Some(2));
    }
}

fn repository(path: &Path) {
    fs::create_dir_all(path).unwrap();
    assert!(
        isolated("git")
            .args(["init", "-q"])
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
    assert!(prepares[0].ends_with(&format!("custom dogsled host prepare --repo 'repo space' --branch feature --request-id {id} --base main --budget-ms 600000 --json")), "{ssh}");
    let releases: Vec<_> = ssh
        .lines()
        .filter(|line| line.contains("host prepared "))
        .collect();
    assert_eq!(releases.len(), 1, "{ssh}");
    assert!(releases[0].ends_with(&format!("custom dogsled host prepared --path /remote/tree --request-id {id} --budget-ms 600000 --json")), "{ssh}");
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
    rig.snapshot("state/host.json", now(), 1.0, 2);
    let mut child = rig
        .cmd(&["status", "--watch", "--line", "--interval", "3600"])
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
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
    child.kill().unwrap();
    child.wait().unwrap();
    reader.join().unwrap();
    assert_eq!(frame.unwrap(), "local 1/2");
}
