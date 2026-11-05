//! `dogsled lead` and the MCP `lead` tool: one ownerless Pi lead, prompted once.

use super::close::git;
use super::*;
use std::io::{BufRead as _, BufReader, Write as _};
use std::os::unix::net::UnixListener;
use std::sync::{Arc, Mutex};

const ORIGIN: &str = "https://example.org/repo.git";

/// A repository checkout registered in the rig's config, shown by no Herdr workspace.
fn registered(rig: &Rig) -> PathBuf {
    let repo = rig.dir.join("home/repo");
    fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["remote", "add", "origin", ORIGIN]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "init"]);
    configure(rig, &[("~/repo", ORIGIN)]);
    rig.put("herdr-worktrees", &json!({"result":{"worktrees":[]}}));
    repo.canonicalize().unwrap()
}

fn configure(rig: &Rig, repos: &[(&str, &str)]) {
    let text = repos
        .iter()
        .fold(String::new(), |mut text, (path, origin)| {
            use std::fmt::Write as _;
            let _ = write!(
                text,
                "[[repos]]\npath = \"{path}\"\norigin = \"{origin}\"\n"
            );
            text
        });
    fs::write(rig.dir.join("config.toml"), text).unwrap();
}

/// A Herdr API socket that records every request. `agent.prompt` fails while
/// `$RIG/prompt-error` exists.
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
            .map(|request| request["method"].as_str().unwrap().to_owned())
            .collect()
    }

    fn prompts(&self) -> Vec<Value> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|request| request["method"] == "agent.prompt")
            .map(|request| request["params"].clone())
            .collect()
    }
}

/// `dogsled lead` with no caller pane or workspace, against the socket.
fn lead(rig: &Rig, socket: &Socket, repo: &str, message: &str) -> (i32, Value) {
    let mut command = rig.cmd(&["--json", "lead", "--repo", repo, "--message", message]);
    command
        .env_remove("HERDR_PANE_ID")
        .env_remove("HERDR_WORKSPACE_ID")
        .env("HERDR_SOCKET_PATH", &socket.path);
    let output = command.output().unwrap();
    // `--json` reports a failure as `{"error": ...}` on stdout.
    let text = String::from_utf8_lossy(&output.stdout);
    let value = serde_json::from_str(text.trim()).unwrap_or_else(|_| json!(text.trim()));
    (output.status.code().unwrap_or(-1), value)
}

fn leads(rig: &Rig) -> Vec<Value> {
    let database = rig.database();
    let mut statement = database
        .prepare("SELECT id, name, phase, pane, terminal, workspace, error, closed_ms FROM leads")
        .unwrap();
    statement
        .query_map([], |row| {
            Ok(json!({
                "id": row.get::<_, String>(0)?,
                "name": row.get::<_, String>(1)?,
                "phase": row.get::<_, String>(2)?,
                "pane": row.get::<_, Option<String>>(3)?,
                "terminal": row.get::<_, Option<String>>(4)?,
                "workspace": row.get::<_, Option<String>>(5)?,
                "error": row.get::<_, Option<String>>(6)?,
                "closed_ms": row.get::<_, Option<i64>>(7)?,
            }))
        })
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

fn starts(rig: &Rig) -> Vec<String> {
    rig.log("herdr.log")
        .lines()
        .filter(|line| line.starts_with("agent start"))
        .map(str::to_owned)
        .collect()
}

#[test]
fn lead_starts_one_ownerless_pi_prompts_once_and_closes_only_its_pane() {
    let rig = Rig::short("lead-start");
    let repo = registered(&rig);
    let socket = Socket::start(&rig);
    let prompt = "  Plan the release.\nKeep notes.\n";
    let (code, started) = lead(&rig, &socket, "https://example.org/repo", prompt);
    assert_eq!(code, 0, "{started}");
    let name = started["name"].as_str().unwrap().to_owned();
    assert!(name.starts_with("lead-r-") && name.len() <= 32, "{name}");
    assert_eq!(started["pane"], "w2:p1");
    // The repository's own new workspace, never the caller's.
    let log = rig.log("herdr.log");
    assert!(
        log.contains(&format!(
            "workspace create --cwd {} --no-focus",
            repo.display()
        )),
        "{log}"
    );
    assert!(!log.contains("tab create"), "{log}");
    // The exact native argv: no child contract, seed, or profile after `--`.
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
    // No child work, queue entry, or reservation.
    assert_eq!(rig.items(), json!([]));
    assert_eq!(rig.tickets(), json!([]));
    // Closing by name ends only the pane: the repository and its workspace stay.
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
fn invalid_selectors_and_prompts_record_and_start_nothing() {
    let rig = Rig::short("lead-invalid");
    let repo = registered(&rig);
    let socket = Socket::start(&rig);
    for (config, selector, message, expected) in [
        (
            vec![("~/repo", ORIGIN)],
            "https://example.org/other",
            "go",
            "not registered",
        ),
        (vec![("~/repo", ORIGIN)], "~/repo", "  \n", "empty"),
        (
            vec![("~/repo", ORIGIN), ("~/copy", ORIGIN)],
            ORIGIN,
            "go",
            "ambiguous",
        ),
        (
            vec![("~/../repo", ORIGIN)],
            ORIGIN,
            "go",
            "leaves the home directory",
        ),
        (
            vec![("~/repo", "https://example.org/elsewhere.git")],
            "~/repo",
            "go",
            "not the registered",
        ),
        (
            vec![("~/missing", ORIGIN)],
            ORIGIN,
            "go",
            "not a repository",
        ),
    ] {
        configure(&rig, &config);
        let (code, error) = lead(&rig, &socket, selector, message);
        assert_ne!(code, 0, "{selector}: {error}");
        assert!(error.to_string().contains(expected), "{expected}: {error}");
    }
    // No prompt connection: nothing is recorded or allocated either.
    configure(&rig, &[("~/repo", ORIGIN)]);
    let missing = Socket {
        path: rig.dir.join("absent.sock"),
        requests: Arc::default(),
    };
    let (code, error) = lead(&rig, &missing, ORIGIN, "go");
    assert_ne!(code, 0, "{error}");
    assert_eq!(leads(&rig), Vec::<Value>::new());
    assert!(!rig.log("herdr.log").contains("create"));
    assert_eq!(starts(&rig), Vec::<String>::new());
    assert_eq!(socket.prompts(), Vec::<Value>::new());
    assert!(repo.exists());
}

#[test]
fn partial_failures_keep_the_pane_phase_and_cause_without_retrying() {
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
        registered(&rig);
        let socket = Socket::start(&rig);
        rig.script(
            "herdr",
            &HERDR_SCRIPT.replace("case \"$*\" in", &format!("case \"$*\" in\n{script}")),
        );
        let mut command = rig.cmd(&["--json", "lead", "--repo", ORIGIN, "--message", "go"]);
        command
            .env_remove("HERDR_PANE_ID")
            .env_remove("HERDR_WORKSPACE_ID")
            .env("HERDR_SOCKET_PATH", &socket.path)
            .env("DOGSLED_TEST_BUSY_WINDOW_MS", "300");
        let output = command.output().unwrap();
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
        // The pane stays, and nothing is prompted.
        assert!(!rig.log("herdr.log").contains("close"), "{name}");
        assert_eq!(socket.prompts(), Vec::<Value>::new(), "{name}");
    }
    // A refused prompt is reported once and never resent.
    let rig = Rig::short("lead-prompt");
    registered(&rig);
    let socket = Socket::start(&rig);
    fs::write(rig.dir.join("prompt-error"), "").unwrap();
    let (code, error) = lead(&rig, &socket, ORIGIN, "go");
    assert_ne!(code, 0, "{error}");
    assert!(error.to_string().contains("prompt refused"), "{error}");
    assert!(error.to_string().contains("phase prompting"), "{error}");
    assert_eq!(socket.prompts().len(), 1);
    assert_eq!(starts(&rig).len(), 1);
    let saved = leads(&rig);
    assert_eq!(saved[0]["phase"], "prompting");
    assert!(!rig.log("herdr.log").contains("close"));
}

#[test]
fn mcp_lead_uses_the_same_creator_without_stdin_prompts() {
    let rig = Rig::short("lead-mcp");
    registered(&rig);
    let socket = Socket::start(&rig);
    let mut client =
        super::mcp::Client::start_with(&rig, &[("HERDR_SOCKET_PATH", socket.path.as_path())]);
    let (refused, error) = client.call(
        "lead",
        &json!({"repo":ORIGIN,"message":"go","message_file":"-"}),
    );
    assert!(error, "{refused}");
    assert_eq!(leads(&rig), Vec::<Value>::new());
    // A repository registered after the server started is found: the config is reloaded.
    configure(&rig, &[]);
    let (refused, error) = client.call("lead", &json!({"repo":ORIGIN,"message":"go"}));
    assert!(error, "{refused}");
    configure(&rig, &[("~/repo", ORIGIN)]);
    let (started, error) = client.call("lead", &json!({"repo":ORIGIN,"message":"first"}));
    assert!(!error, "{started}");
    assert_eq!(started["pane"], "w2:p1");
    assert_eq!(socket.prompts(), [json!({"target":"w2:p1","text":"first"})]);
    assert_eq!(starts(&rig).len(), 1);
    assert_eq!(leads(&rig)[0]["phase"], "ready");
}
