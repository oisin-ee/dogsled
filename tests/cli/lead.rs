//! Slug-based lead creation uses one published host choice and one native start.

use super::close::git;
use super::*;
use std::io::{BufRead as _, BufReader, Write as _};
use std::os::unix::net::UnixListener;
use std::sync::{Arc, Mutex};

const SLUG: &str = "owner/repo";
const ORIGIN: &str = "git@github.com:owner/repo.git";

pub(super) fn checkout(rig: &Rig, name: &str, origin: &str) -> PathBuf {
    let repo = rig.dir.join("home/dev").join(name);
    fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["remote", "add", "origin", origin]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "init"]);
    rig.put("herdr-worktrees", &json!({"result":{"worktrees":[]}}));
    repo.canonicalize().unwrap()
}

fn observer(rig: &Rig) -> super::observation::Supplied {
    rig.snapshot("state", now(), 0.0, 8);
    super::observation::seeded(rig)
}

/// Records Herdr socket requests, and refuses prompts while prompt-error exists.
struct Socket {
    path: PathBuf,
    requests: Arc<Mutex<Vec<Value>>>,
}

impl Socket {
    fn start(rig: &Rig) -> Self {
        let path = rig.dir.join("h.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&requests);
        let failure = rig.dir.join("prompt-error");
        std::thread::spawn(move || {
            while let Ok((mut stream, _)) = listener.accept() {
                let mut line = String::new();
                if BufReader::new(&stream).read_line(&mut line).is_err() {
                    continue;
                }
                let request: Value = serde_json::from_str(&line).unwrap();
                seen.lock().unwrap().push(request.clone());
                let mut reply = match request["method"].as_str().unwrap() {
                    "agent.list" => json!({"result":{"agents":[]}}),
                    "agent.prompt" if failure.exists() => {
                        json!({"error":{"code":"agent_blocked","message":"prompt refused"}})
                    }
                    "agent.prompt" => json!({"result":{"type":"agent_prompted"}}),
                    other => json!({"error":{"code":"unexpected","message":other}}),
                };
                reply["id"] = request["id"].clone();
                let _ = writeln!(stream, "{reply}");
            }
        });
        Self { path, requests }
    }

    fn methods(&self) -> Vec<String> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .map(|r| r["method"].as_str().unwrap().to_owned())
            .collect()
    }

    fn prompts(&self) -> Vec<Value> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r["method"] == "agent.prompt")
            .map(|r| r["params"].clone())
            .collect()
    }
}

fn lead(rig: &Rig, socket: &Socket, repo: &str, message: &str) -> (i32, Value) {
    let mut command = rig.cmd(&["--json", "lead", repo, message]);
    command
        .env_remove("HERDR_PANE_ID")
        .env_remove("HERDR_WORKSPACE_ID")
        .env("HERDR_SOCKET_PATH", &socket.path);
    let output = command.output().unwrap();
    let text = String::from_utf8_lossy(&output.stdout);
    let value = serde_json::from_str(text.trim()).unwrap_or_else(|_| json!(text.trim()));
    (output.status.code().unwrap_or(-1), value)
}

fn records(rig: &Rig, state: &str) -> Vec<Value> {
    let database = if state == "state" {
        rig.database()
    } else {
        rusqlite::Connection::open_with_flags(
            rig.dir.join(state).join("state.db"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap()
    };
    let mut statement = database
        .prepare("SELECT id, name, phase, pane, terminal, error, closed_ms FROM leads")
        .unwrap();
    statement.query_map([], |row| Ok(json!({
        "id": row.get::<_, String>(0)?, "name": row.get::<_, String>(1)?,
        "phase": row.get::<_, String>(2)?, "pane": row.get::<_, Option<String>>(3)?,
        "terminal": row.get::<_, Option<String>>(4)?, "error": row.get::<_, Option<String>>(5)?,
        "closed_ms": row.get::<_, Option<i64>>(6)?,
    }))).unwrap().map(Result::unwrap).collect()
}

fn leads(rig: &Rig) -> Vec<Value> {
    records(rig, "state")
}

fn starts(rig: &Rig) -> Vec<String> {
    rig.log("herdr.log")
        .lines()
        .filter(|line| line.starts_with("agent start"))
        .map(str::to_owned)
        .collect()
}

#[test]
fn lead_reuses_matching_main_checkout_preserves_branch_and_prompts_once() {
    let rig = Rig::short("lead-start");
    let repo = checkout(&rig, "custom-name", "https://github.com/owner/repo.git");
    git(&repo, &["switch", "-q", "-c", "in-progress"]);
    let _observer = observer(&rig);
    let socket = Socket::start(&rig);
    let prompt = "  Plan the release.\nKeep notes.\n";
    let (code, started) = lead(&rig, &socket, SLUG, prompt);
    assert_eq!(code, 0, "{started}");
    let name = started["name"].as_str().unwrap().to_owned();
    assert!(name.starts_with("lead-r-") && name.len() <= 32, "{name}");
    assert_eq!(started["host"], "local");
    assert_eq!(started["pane"], "w2:p1");
    let log = rig.log("herdr.log");
    assert!(
        log.contains(&format!(
            "workspace create --cwd {} --no-focus",
            repo.display()
        )),
        "{log}"
    );
    assert!(!log.contains("tab create"), "{log}");
    assert_eq!(
        starts(&rig),
        [format!("agent start {name} --kind pi --pane w2:p1")]
    );
    assert_eq!(rig.log("typed.log"), "pi\n");
    assert_eq!(socket.prompts(), [json!({"target":"w2:p1","text":prompt})]);
    assert_eq!(socket.methods(), ["agent.list", "agent.prompt"]);
    let saved = leads(&rig);
    assert_eq!(saved.len(), 1, "{saved:?}");
    assert_eq!(saved[0]["name"], json!(name));
    assert_eq!(saved[0]["phase"], "ready");
    assert_eq!(saved[0]["terminal"], "term_child");
    assert_eq!(saved[0]["error"], Value::Null);
    assert_eq!(rig.items(), json!([]));
    assert_eq!(rig.tickets(), json!([]));
    assert_eq!(
        git(&repo, &["branch", "--show-current"]).trim(),
        "in-progress"
    );
    let (code, closed) = rig.json(&["close", &name]);
    assert_eq!(code, 0, "{closed}");
    assert_eq!(closed["pane"], "w2:p1");
    assert_eq!(closed["worktree"], Value::Null);
    let log = rig.log("herdr.log");
    assert!(log.contains("pane close w2:p1"), "{log}");
    assert!(!log.contains("workspace close"), "{log}");
    assert!(repo.join(".git").exists());
    assert!(leads(&rig)[0]["closed_ms"].is_i64());
    assert_eq!(socket.prompts().len(), 1);
}

#[test]
fn invalid_slugs_and_prompts_start_nothing() {
    let rig = Rig::short("lead-invalid");
    let socket = Socket::start(&rig);
    // No observer or local checkout is needed to reject invalid input.
    for selector in [
        "",
        "repo",
        "/repo",
        "owner/",
        "owner/repo/extra",
        "../repo",
        "owner/..",
        "https://github.com/owner/repo",
        "owner/repo;true",
        "owner/repo\n",
    ] {
        let (code, error) = lead(&rig, &socket, selector, "go");
        assert_ne!(code, 0, "{selector}: {error}");
        assert!(error.to_string().contains("owner/name"), "{error}");
    }
    let (code, error) = lead(&rig, &socket, SLUG, "  \n");
    assert_ne!(code, 0, "{error}");
    assert!(error.to_string().contains("empty"), "{error}");
    assert_eq!(leads(&rig), Vec::<Value>::new());
    assert!(!rig.dir.join("home/dev").exists());
    assert_eq!(starts(&rig), Vec::<String>::new());
    assert_eq!(socket.methods(), Vec::<String>::new());
    assert_eq!(rig.log("ssh.log"), "");
}

#[test]
fn lead_unavailable_socket_does_not_allocate_or_clone() {
    let rig = Rig::short("lead-no-socket");
    let _observer = observer(&rig);
    let missing = Socket {
        path: rig.dir.join("absent.sock"),
        requests: Arc::default(),
    };
    let (code, error) = lead(&rig, &missing, SLUG, "go");
    assert_ne!(code, 0, "{error}");
    assert_eq!(leads(&rig), Vec::<Value>::new());
    assert!(!rig.log("herdr.log").contains("create"));
    assert!(!rig.dir.join("home/dev").exists());
}

#[test]
fn lead_partial_failures_keep_phase_and_cause_without_retrying() {
    let busy = "  'agent start'*) echo '{\"error\":{\"code\":\"agent_pane_busy\",\"message\":\"not an available shell\"}}' >&2; exit 1 ;;";
    let refused = "  'agent start'*) echo '{\"error\":{\"code\":\"agent_start_failed\",\"message\":\"pi exited\"}}' >&2; exit 1 ;;\n  'pane read'*) printf 'pi: missing model\\n' ;;";
    let replaced = "  'agent start'*) sed 's/term_child/term_other/' \"$RIG/panes.json\" > \"$RIG/p\" && mv \"$RIG/p\" \"$RIG/panes.json\"; echo '{\"error\":{\"code\":\"agent_pane_busy\",\"message\":\"not an available shell\"}}' >&2; exit 1 ;;";
    for (name, script, expected) in [
        ("lead-busy", busy, "still not an available shell"),
        ("lead-refused", refused, "pi: missing model"),
        (
            "lead-replaced",
            replaced,
            "launch terminal could not be revalidated",
        ),
    ] {
        let rig = Rig::short(name);
        checkout(&rig, "repo", ORIGIN);
        let _observer = observer(&rig);
        let socket = Socket::start(&rig);
        rig.script(
            "herdr",
            &HERDR_SCRIPT.replace("case \"$*\" in", &format!("case \"$*\" in\n{script}")),
        );
        let output = rig
            .cmd(&["--json", "lead", SLUG, "go"])
            .env("HERDR_SOCKET_PATH", &socket.path)
            .env("DOGSLED_TEST_BUSY_WINDOW_MS", "300")
            .output()
            .unwrap();
        let error = String::from_utf8_lossy(&output.stdout);
        assert!(!output.status.success(), "{name}: {output:?}");
        assert!(error.contains(expected), "{name}: {error}");
        assert!(
            error.contains("pane w2:p1") && error.contains("phase starting"),
            "{error}"
        );
        let saved = leads(&rig);
        assert_eq!(saved.len(), 1, "{name}: {saved:?}");
        assert_eq!(saved[0]["phase"], "starting");
        assert_eq!(saved[0]["pane"], "w2:p1");
        assert!(
            saved[0]["error"].as_str().unwrap().contains(expected),
            "{saved:?}"
        );
        assert!(error.contains(saved[0]["id"].as_str().unwrap()), "{error}");
        assert!(!rig.log("herdr.log").contains("close"), "{name}");
        assert_eq!(socket.prompts(), Vec::<Value>::new(), "{name}");
    }
    let rig = Rig::short("lead-prompt");
    checkout(&rig, "repo", ORIGIN);
    let _observer = observer(&rig);
    let socket = Socket::start(&rig);
    fs::write(rig.dir.join("prompt-error"), "").unwrap();
    let (code, error) = lead(&rig, &socket, SLUG, "go");
    assert_ne!(code, 0, "{error}");
    assert!(error.to_string().contains("prompt refused"), "{error}");
    assert!(error.to_string().contains("phase prompting"), "{error}");
    assert_eq!(socket.prompts().len(), 1);
    assert_eq!(starts(&rig).len(), 1);
    assert_eq!(leads(&rig)[0]["phase"], "prompting");
    assert!(!rig.log("herdr.log").contains("close"));
}

#[test]
fn mcp_lead_uses_the_same_slug_creator_without_stdin_prompts() {
    let rig = Rig::short("lead-mcp");
    checkout(&rig, "repo", ORIGIN);
    let _observer = observer(&rig);
    let socket = Socket::start(&rig);
    let mut client =
        super::mcp::Client::start_with(&rig, &[("HERDR_SOCKET_PATH", socket.path.as_path())]);
    let (refused, error) = client.call(
        "lead",
        &json!({"repo":SLUG,"message":"go","message_file":"-"}),
    );
    assert!(error, "{refused}");
    assert_eq!(leads(&rig), Vec::<Value>::new());
    let (refused, error) = client.call("lead", &json!({"repo":ORIGIN,"message":"go"}));
    assert!(error, "{refused}");
    let (started, error) = client.call("lead", &json!({"repo":SLUG,"message":"first"}));
    assert!(!error, "{started}");
    assert_eq!(started["host"], "local");
    assert_eq!(started["pane"], "w2:p1");
    assert_eq!(socket.prompts(), [json!({"target":"w2:p1","text":"first"})]);
    assert_eq!(starts(&rig).len(), 1);
    assert_eq!(leads(&rig)[0]["phase"], "ready");
}

/// An offline source plus a POSIX git fixture exercises the real clone operation without GitHub.
pub(super) fn clone_source(rig: &Rig) {
    let source = checkout(rig, "source", "git@github.com:other/source.git");
    let real_git = String::from_utf8(
        isolated("sh")
            .args(["-c", "command -v git"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    rig.script(
        "git",
        &format!(
            r#"#!/bin/sh
printf '%s\n' "$*" >> "$RIG/git.log"
if test "$1" = clone; then
    origin=$2
    destination=$3
    "{}" clone '{}' "$destination" || exit
    "{}" -C "$destination" remote set-url origin "$origin"
else
    exec "{}" "$@"
fi
"#,
            real_git.trim(),
            source.display(),
            real_git.trim(),
            real_git.trim()
        ),
    );
}

#[test]
fn lead_clones_missing_checkout_and_uses_owner_prefix_for_a_name_clash() {
    for clash in [false, true] {
        let rig = Rig::short("lead-clone");
        clone_source(&rig);
        let other = clash.then(|| checkout(&rig, "repo", "git@github.com:other/repo.git"));
        let _observer = observer(&rig);
        let socket = Socket::start(&rig);
        let (code, started) = lead(&rig, &socket, SLUG, "go");
        assert_eq!(code, 0, "{started}");
        let destination = rig.dir.join(if clash {
            "home/dev/owner-repo"
        } else {
            "home/dev/repo"
        });
        assert!(destination.join(".git").is_dir());
        let log = rig.log("git.log");
        assert!(
            log.contains(&format!("clone {ORIGIN} {}", destination.display())),
            "{log}"
        );
        assert_eq!(
            log.lines()
                .filter(|line| line.starts_with("clone "))
                .count(),
            1
        );
        if let Some(other) = other {
            assert_eq!(
                git(&other, &["remote", "get-url", "origin"]).trim(),
                "git@github.com:other/repo.git"
            );
        }
    }
}

pub(super) fn remote_publication(rig: &Rig) -> super::observation::Supplied {
    let local_observer = observer(rig);
    let (_, mut frame) = rig.json(&["observation", "current"]);
    drop(local_observer);
    let mut local = frame["views"]["hosts"]["data"][0].clone();
    local["headroom"]["eligible"] = json!(false);
    local["headroom"]["headroom"] = json!(-8.0);
    let mut peer = local.clone();
    peer["id"] = json!("8103");
    peer["label"] = json!("netcup");
    peer["target"] = json!("peer-host");
    peer["headroom"]["host"] = json!("8103");
    peer["headroom"]["headroom"] = json!(-1.0);
    let mut unreachable = peer.clone();
    unreachable["id"] = json!("offline");
    unreachable["label"] = json!("offline");
    unreachable["reachable"] = json!(false);
    unreachable["headroom"]["host"] = json!("offline");
    unreachable["headroom"]["headroom"] = json!(100.0);
    frame["views"]["hosts"]["data"] = json!([local, peer, unreachable]);
    super::observation::Supplied::start(rig, frame)
}

pub(super) fn peer_transport(rig: &Rig, lose_reply: bool) {
    rig.script(
        "herdr",
        &HERDR_SCRIPT
            .replace("w2:p1", "w9:p1")
            .replace("term_child", "term_peer"),
    );
    rig.put(
        "panes.json",
        &json!({"result":{"panes":[{"pane_id":"w9:p1","terminal_id":"term_peer"}]}}),
    );
    rig.script("ssh", &format!(r#"#!/bin/sh
printf '%s\n' "$*" >> "$RIG/ssh.log"
for remote do :; done
case "$remote" in
  *'host lead --input -'*)
    tee "$RIG/lead-delivery.json" | HOME="$RIG/peer-home" DOGSLED_STATE="$RIG/peer-state" sh -c '"$DOGSLED_BIN" '"$remote" > "$RIG/lead-reply.json"
    {}
    cat "$RIG/lead-reply.json" ;;
  *'host repo-open --repo '*)
    HOME="$RIG/peer-home" DOGSLED_STATE="$RIG/peer-state" sh -c '"$DOGSLED_BIN" '"$remote" ;;
  *) exit 97 ;;
esac
"#, if lose_reply { "exit 255" } else { ":" }));
    fs::create_dir_all(rig.dir.join("peer-home")).unwrap();
    fs::write(rig.dir.join("config.toml"), "remote_command = []\n").unwrap();
}

#[test]
fn lead_chooses_roomiest_over_limit_host_and_starts_through_peer_ingress() {
    let rig = Rig::short("lead-remote");
    clone_source(&rig);
    let _observer = remote_publication(&rig);
    let socket = Socket::start(&rig);
    peer_transport(&rig, false);
    let (code, started) = lead(&rig, &socket, SLUG, "  remote\nfirst prompt\n");
    assert_eq!(code, 0, "{started}");
    assert_eq!(started["host"], "netcup");
    assert_eq!(started["pane"], "w9:p1");
    assert_eq!(leads(&rig), Vec::<Value>::new());
    assert_eq!(records(&rig, "peer-state")[0]["phase"], "ready");
    assert!(rig.dir.join("peer-home/dev/repo/.git").is_dir());
    assert!(!rig.dir.join("home/dev/repo").exists());
    assert_eq!(
        socket.prompts(),
        [json!({"target":"w9:p1","text":"  remote\nfirst prompt\n"})]
    );
    let log = rig.log("ssh.log");
    assert_eq!(log.lines().count(), 1, "{log}");
    assert!(
        log.contains("peer-host") && log.contains("host lead --input -"),
        "{log}"
    );
    assert!(!log.contains("first prompt"), "{log}");
    let mut delivery = rig.read("lead-delivery.json");
    let selected: String = rig
        .database()
        .query_row(
            "SELECT host FROM placement_attempts WHERE id = ?1",
            [delivery["id"].as_str().unwrap()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(selected, "netcup");
    // Completed replay is read-only; a changed host cannot change its saved receipt.
    let replay = peer_call(&rig, &socket, &delivery);
    assert_eq!(replay["outcome"], "started");
    assert_eq!(replay["lead"], started);
    delivery["host"] = json!("other");
    assert_eq!(peer_call(&rig, &socket, &delivery)["outcome"], "failed");
    assert_eq!(starts(&rig).len(), 1);
    assert_eq!(socket.prompts().len(), 1);
}

fn peer_call(rig: &Rig, socket: &Socket, delivery: &Value) -> Value {
    let mut child = rig
        .cmd(&["--json", "host", "lead", "--input", "-"])
        .env("HOME", rig.dir.join("peer-home"))
        .env("DOGSLED_STATE", rig.dir.join("peer-state"))
        .env("HERDR_SOCKET_PATH", &socket.path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    serde_json::to_writer(child.stdin.take().unwrap(), delivery).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "{output:?}");
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn lead_peer_prompt_failure_preserves_phase_and_never_reprompts() {
    let rig = Rig::short("lead-peer-fail");
    clone_source(&rig);
    let _observer = remote_publication(&rig);
    let socket = Socket::start(&rig);
    peer_transport(&rig, false);
    fs::write(rig.dir.join("prompt-error"), "").unwrap();
    let (code, error) = lead(&rig, &socket, SLUG, "go");
    assert_ne!(code, 0, "{error}");
    assert!(error.to_string().contains("phase prompting"), "{error}");
    assert!(error.to_string().contains("pane w9:p1"), "{error}");
    let delivery = rig.read("lead-delivery.json");
    let replay = peer_call(&rig, &socket, &delivery);
    assert_eq!(replay["outcome"], "failed");
    assert_eq!(replay["phase"], "prompting");
    assert_eq!(socket.prompts().len(), 1);
    assert_eq!(starts(&rig).len(), 1);
    assert_eq!(leads(&rig), Vec::<Value>::new());
}

#[test]
fn lead_lost_peer_reply_does_not_retry_or_fall_back_to_local() {
    let rig = Rig::short("lead-uncertain");
    clone_source(&rig);
    let _observer = remote_publication(&rig);
    let socket = Socket::start(&rig);
    peer_transport(&rig, true);
    let (code, error) = lead(&rig, &socket, SLUG, "go");
    assert_ne!(code, 0, "{error}");
    assert!(error.to_string().contains("uncertain"), "{error}");
    assert_eq!(rig.log("ssh.log").lines().count(), 1);
    assert_eq!(starts(&rig).len(), 1);
    assert_eq!(socket.prompts().len(), 1);
    assert_eq!(leads(&rig), Vec::<Value>::new());
    assert_eq!(records(&rig, "peer-state")[0]["phase"], "ready");
}
