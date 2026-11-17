use super::*;
use crate::shell::Output;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Debug)]
struct StartingShell {
    starts: AtomicUsize,
}

fn validity_for(ledger: &Ledger, id: &str) -> crate::jobs::Validity {
    let validity = crate::jobs::origin_validity(ledger).unwrap();
    crate::jobs::record_request_validity(ledger, id, Some(validity)).unwrap();
    validity
}

impl Shell for StartingShell {
    fn run(&self, program: &str, args: &[String]) -> Result<Output> {
        assert_eq!(program, "herdr");
        let value = match args
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .as_slice()
        {
            ["pane", "list"] => json!({"result":{"panes":[{
                "pane_id":"w2:p2","terminal_id":"term_shell"
            }]}}),
            ["agent", "list"] => json!({"result":{"agents":[]}}),
            ["pane", "read", ..] => {
                return Err(Error::Invalid("startup output unavailable".into()));
            }
            ["agent", "start", ..] => {
                self.starts.fetch_add(1, Ordering::SeqCst);
                return Err(Error::Invalid("test stops at native start".into()));
            }
            _ => panic!("unexpected command {args:?}"),
        };
        Ok(Output {
            status: 0,
            stdout: value.to_string(),
            stderr: String::new(),
        })
    }
}

#[test]
fn another_request_cannot_start_while_the_pane_is_locked_for_recovery() {
    let mut ledger = Ledger::new(std::env::temp_dir().join(crate::ledger::new_id()));
    let progress: Progress = serde_json::from_value(json!({
        "request":{"id":"different","validity":validity_for(&ledger,"different"),"action":{"type":"agent","kind":"pi","name":"worker","message":"go"},
            "host":"local","cwd":"/checkout","pane":"w2:p2","terminal":"term_shell","owner":"w1:p1"},
        "pane":"w2:p2","terminal":"term_shell","attempted":false,"confirmed":false,
        "result":{},"error":null
    })).unwrap();
    crate::launch_store::put_progress_value(&ledger, "different", &progress).unwrap();
    let panes = ledger.child("launch-panes");
    std::fs::create_dir_all(&panes.dir).unwrap();
    let key = crate::unit_transport::digest_hex(b"w2:p2");
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(panes.dir.join(format!("{key}.lock")))
        .unwrap();
    lock.lock().unwrap();
    ledger.budget = crate::budget::Budget::until(
        std::time::Instant::now() + std::time::Duration::from_millis(30),
    );
    let shell = StartingShell {
        starts: AtomicUsize::new(0),
    };
    assert!(matches!(
        agent(&ledger, &shell, &progress.request),
        Err(Error::Deadline(_))
    ));
    assert_eq!(shell.starts.load(Ordering::SeqCst), 0);
    ledger.budget = crate::budget::Budget::default();
    assert!(
        !crate::launch_store::progress(&ledger, "different")
            .unwrap()
            .unwrap()
            .attempted
    );
    drop(lock);
    ledger.budget = crate::budget::Budget::default();
    assert!(matches!(
        agent(&ledger, &shell, &progress.request),
        Err(Error::Invalid(_))
    ));
    assert_eq!(shell.starts.load(Ordering::SeqCst), 1);
    let unconfirmed = crate::launch_store::progress(&ledger, "different")
        .unwrap()
        .unwrap();
    assert!(!unconfirmed.confirmed);
    assert!(unconfirmed.result.get("launch_confirmed_ms").is_none());
    std::fs::remove_dir_all(ledger.dir).unwrap();
}

#[derive(Debug)]
struct ConfirmingShell {
    recovering: bool,
    starts: AtomicUsize,
}

impl Shell for ConfirmingShell {
    fn run(&self, program: &str, args: &[String]) -> Result<Output> {
        if program == "git" {
            assert_eq!(
                args,
                &[
                    "-C",
                    "/",
                    "rev-parse",
                    "--path-format=absolute",
                    "--git-common-dir"
                ]
            );
            return Ok(Output {
                status: 128,
                stdout: String::new(),
                stderr: "not a git repository".into(),
            });
        }
        assert_eq!(program, "herdr");
        let value = match args
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .as_slice()
        {
            ["pane", "list"] => json!({"result":{"panes":[{
                "pane_id":"w2:p2","terminal_id":"term_shell"
            }]}}),
            ["agent", "list"] if self.recovering => json!({"result":{"agents":[{
                "pane_id":"w2:p2","terminal_id":"term_shell","name":"worker","agent":"codex"
            }]}}),
            ["agent", "list"] => json!({"result":{"agents":[]}}),
            ["agent", "start", ..] => {
                self.starts.fetch_add(1, Ordering::SeqCst);
                json!({"result":{}})
            }
            ["pane", "read", ..] => {
                return Err(Error::Invalid("startup output unavailable".into()));
            }
            _ => panic!("unexpected command {args:?}"),
        };
        Ok(Output {
            status: 0,
            stdout: value.to_string(),
            stderr: String::new(),
        })
    }
}

#[test]
fn confirmation_is_stamped_after_start_or_recovery_and_preserved_on_replay() {
    for recovering in [false, true] {
        let ledger = Ledger::new(std::env::temp_dir().join(crate::ledger::new_id()));
        let progress: Progress = serde_json::from_value(json!({
            "request":{"id":"stamp","validity":validity_for(&ledger,"stamp"),"action":{"type":"agent","kind":"codex","name":"worker","message":"go"},
                "host":"peer","cwd":"/","pane":"w2:p2","terminal":"term_shell","owner":"parent"},
            "pane":"w2:p2","terminal":"term_shell","attempted":recovering,"confirmed":false,
            "result":{"host":"peer","pane":"w2:p2"},"error":null
        })).unwrap();
        crate::launch_store::put_progress_value(&ledger, "stamp", &progress).unwrap();
        let shell = ConfirmingShell {
            recovering,
            starts: AtomicUsize::new(0),
        };
        let before = crate::ledger::now_ms();
        // The caller's old admission timestamp must not become confirmation time.
        let result = work::launch(
            &ledger,
            &shell,
            &progress.request,
            1,
            Some(crate::jobs::REQUEST_VALIDITY_PROTOCOL_VERSION),
        )
        .unwrap();
        let confirmed = result["launch_confirmed_ms"].as_u64().unwrap();
        assert!(confirmed >= before && confirmed <= crate::ledger::now_ms());
        assert_eq!(
            shell.starts.load(Ordering::SeqCst),
            usize::from(!recovering)
        );
        let saved = crate::launch_store::progress(&ledger, "stamp")
            .unwrap()
            .unwrap();
        assert!(saved.confirmed);
        assert_eq!(saved.result, result);
        // Simulate interruption between confirmation and final receipt publication.
        crate::launch_store::remove(&ledger, crate::launch_store::Record::Receipt, "stamp")
            .unwrap();
        assert_eq!(agent(&ledger, &shell, &progress.request).unwrap(), result);
        assert_eq!(agent(&ledger, &shell, &progress.request).unwrap(), result);
        assert_eq!(
            shell.starts.load(Ordering::SeqCst),
            usize::from(!recovering)
        );
        std::fs::remove_dir_all(ledger.dir).unwrap();
    }
}

#[derive(Debug)]
struct BusyShell {
    busy: usize,
    starts: AtomicUsize,
    /// After this many starts, the pane id belongs to a replacement terminal.
    replaced_after: usize,
}

impl BusyShell {
    fn new(busy: usize) -> Self {
        Self {
            busy,
            starts: AtomicUsize::new(0),
            replaced_after: usize::MAX,
        }
    }
}

impl Shell for BusyShell {
    fn run(&self, _program: &str, args: &[String]) -> Result<Output> {
        if args.starts_with(&["pane".into(), "list".into()]) {
            let terminal = if self.starts.load(Ordering::SeqCst) >= self.replaced_after {
                "term_replacement"
            } else {
                "term_shell"
            };
            return Ok(Output {
                status: 0,
                stdout: json!({"result":{"panes":[{"pane_id":"w2:p2","terminal_id":terminal}]}})
                    .to_string(),
                stderr: String::new(),
            });
        }
        let start = self.starts.fetch_add(1, Ordering::SeqCst);
        Ok(if start < self.busy {
            Output {
                status: 1,
                stdout: String::new(),
                stderr: r#"{"error":{"code":"agent_pane_busy","message":"agent target pane w2:p2 is not an available shell"}}"#.into(),
            }
        } else {
            Output {
                status: 0,
                stdout: "{}".into(),
                stderr: String::new(),
            }
        })
    }
}

fn busy_request(ledger: &Ledger) -> Launch {
    let validity = crate::jobs::origin_validity(ledger).unwrap();
    crate::jobs::record_request_validity(ledger, "busy", Some(validity)).unwrap();
    serde_json::from_value(json!({"id":"busy","validity":validity,
        "action":{"type":"agent","kind":"pi","name":"worker","message":"go"},
        "host":"local","cwd":"/checkout","owner":"parent"}))
    .unwrap()
}

fn start_args() -> Vec<String> {
    vec!["agent".into(), "start".into()]
}

fn start_with_simulated_busy_wait(
    ledger: &Ledger,
    shell: &dyn Shell,
    args: &[String],
    pane: &str,
    terminal: &str,
    window: std::time::Duration,
    request: &Launch,
) -> Result<()> {
    let elapsed = std::cell::Cell::new(std::time::Duration::ZERO);
    start_with_busy_wait(
        ledger,
        shell,
        args,
        pane,
        terminal,
        &BusyWait {
            window,
            elapsed: &|| elapsed.get(),
            pause: &|duration| elapsed.set(elapsed.get() + duration),
        },
        &|| work::check_launch_validity(ledger, request),
    )
}

#[test]
fn a_busy_pane_is_started_again_once_its_shell_is_ready() {
    let ledger = Ledger::new(std::env::temp_dir().join(crate::ledger::new_id()));
    let shell = BusyShell::new(2);
    let window = std::time::Duration::from_secs(5);
    start_with_simulated_busy_wait(
        &ledger,
        &shell,
        &start_args(),
        "w2:p2",
        "term_shell",
        window,
        &busy_request(&ledger),
    )
    .unwrap();
    assert_eq!(shell.starts.load(Ordering::SeqCst), 3);
}

#[test]
fn a_busy_retry_is_refused_once_another_terminal_holds_the_pane() {
    let ledger = Ledger::new(std::env::temp_dir().join(crate::ledger::new_id()));
    let shell = BusyShell {
        replaced_after: 1,
        ..BusyShell::new(usize::MAX)
    };
    let window = std::time::Duration::from_secs(5);
    let error = start_with_simulated_busy_wait(
        &ledger,
        &shell,
        &start_args(),
        "w2:p2",
        "term_shell",
        window,
        &busy_request(&ledger),
    )
    .unwrap_err();
    assert!(matches!(error, Error::Launch(_)), "{error:?}");
    assert!(
        error.to_string().contains("launch terminal changed"),
        "{error}"
    );
    // Only the first, refused start ran; the replacement terminal never got a second one.
    assert_eq!(shell.starts.load(Ordering::SeqCst), 1);
}

#[test]
fn a_pane_busy_past_the_window_names_the_pane_and_the_wait() {
    let ledger = Ledger::new(std::env::temp_dir().join(crate::ledger::new_id()));
    let shell = BusyShell::new(usize::MAX);
    let window = std::time::Duration::from_millis(300);
    let error = start_with_simulated_busy_wait(
        &ledger,
        &shell,
        &start_args(),
        "w2:p2",
        "term_shell",
        window,
        &busy_request(&ledger),
    )
    .unwrap_err();
    let Error::Launch(message) = &error else {
        panic!("expected a launch error, got {error:?}");
    };
    assert!(message.contains("w2:p2"), "{message}");
    assert!(message.contains("after 0.3s"), "{message}");
    assert_eq!(shell.starts.load(Ordering::SeqCst), 3);
}

#[test]
fn a_first_busy_refusal_can_exhaust_the_window_without_a_retry() {
    let ledger = Ledger::new(std::env::temp_dir().join(crate::ledger::new_id()));
    let shell = BusyShell::new(usize::MAX);
    let window = std::time::Duration::from_millis(300);
    let request = busy_request(&ledger);
    // A slow first command or a descheduled caller can consume the whole window.
    let error = start_with_busy_wait(
        &ledger,
        &shell,
        &start_args(),
        "w2:p2",
        "term_shell",
        &BusyWait {
            window,
            elapsed: &|| window,
            pause: &|_| panic!("an exhausted window must not pause"),
        },
        &|| work::check_launch_validity(&ledger, &request),
    )
    .unwrap_err();
    assert!(matches!(error, Error::Launch(_)), "{error:?}");
    assert!(error.to_string().contains("after 0.3s"), "{error}");
    assert_eq!(shell.starts.load(Ordering::SeqCst), 1);
}

#[derive(Debug)]
struct BudgetShell {
    /// Set once a start is refused as busy, so setup (locks, fsync) never races the deadline.
    expired: std::sync::atomic::AtomicBool,
    starts: AtomicUsize,
    uncertain: bool,
}

impl Shell for BudgetShell {
    fn budget(&self) -> crate::budget::Budget {
        if self.expired.load(Ordering::SeqCst) {
            crate::budget::Budget::until(std::time::Instant::now())
        } else {
            crate::budget::Budget::default()
        }
    }

    fn run(&self, program: &str, args: &[String]) -> Result<Output> {
        if args.starts_with(&["pane".into(), "read".into()]) {
            return Err(Error::Invalid("startup output unavailable".into()));
        }
        if args.starts_with(&["agent".into(), "start".into()]) {
            self.starts.fetch_add(1, Ordering::SeqCst);
            if self.uncertain {
                return Err(Error::Deadline("issued agent start".into()));
            }
            let refused = BusyShell::new(usize::MAX).run(program, args);
            self.expired.store(true, Ordering::SeqCst);
            return refused;
        }
        ConfirmingShell {
            recovering: false,
            starts: AtomicUsize::new(0),
        }
        .run(program, args)
    }
}

#[test]
fn command_budget_expiry_after_busy_stays_retryable_but_an_issued_timeout_does_not() {
    for uncertain in [false, true] {
        let ledger = Ledger::new(std::env::temp_dir().join(crate::ledger::new_id()));
        let progress: Progress = serde_json::from_value(json!({
            "request":{"id":"budget","validity":validity_for(&ledger,"budget"),"action":{"type":"agent","kind":"codex","name":"worker","message":"go"},
                "host":"local","cwd":"/","pane":"w2:p2","terminal":"term_shell","owner":"parent"},
            "pane":"w2:p2","terminal":"term_shell","attempted":false,"confirmed":false,
            "result":{"host":"local","pane":"w2:p2"},"error":null
        })).unwrap();
        crate::launch_store::put_progress_value(&ledger, "budget", &progress).unwrap();
        let shell = BudgetShell {
            expired: std::sync::atomic::AtomicBool::new(false),
            starts: AtomicUsize::new(0),
            uncertain,
        };
        assert!(agent(&ledger, &shell, &progress.request).is_err());
        assert_eq!(shell.starts.load(Ordering::SeqCst), 1);
        let saved = crate::launch_store::progress(&ledger, "budget")
            .unwrap()
            .unwrap();
        assert_eq!(saved.attempted, uncertain);
        assert!(!saved.confirmed);
        assert!(
            crate::launch_store::receipt(&ledger, "budget")
                .unwrap()
                .is_none()
        );
        let ready = ConfirmingShell {
            recovering: false,
            starts: AtomicUsize::new(0),
        };
        assert_eq!(
            agent(&ledger, &ready, &progress.request).is_ok(),
            !uncertain
        );
        assert_eq!(ready.starts.load(Ordering::SeqCst), usize::from(!uncertain));
        std::fs::remove_dir_all(ledger.dir).unwrap();
    }
}

#[derive(Debug)]
struct TakenShell;

impl Shell for TakenShell {
    fn run(&self, _program: &str, _args: &[String]) -> Result<Output> {
        Ok(Output {
            status: 1,
            stdout: String::new(),
            stderr: r#"{"error":{"code":"agent_name_taken","message":"taken"}}"#.into(),
        })
    }
}

#[test]
fn only_a_busy_refusal_is_repeated() {
    let ledger = Ledger::new(std::env::temp_dir().join(crate::ledger::new_id()));
    let window = std::time::Duration::from_secs(5);
    let error = start_while_busy(
        &ledger,
        &TakenShell,
        &start_args(),
        "w2:p2",
        "term_shell",
        window,
        &busy_request(&ledger),
    )
    .unwrap_err();
    assert!(matches!(error, Error::Command { .. }), "{error:?}");
}

/// Herdr that times out waiting for the agent it started. The agent comes up later, unnamed,
/// in the launch's terminal once `ready_after` agent lists have run.
#[derive(Debug)]
struct SlowStartShell {
    lists: AtomicUsize,
    ready_after: usize,
    agent: Value,
    calls: std::sync::Mutex<Vec<String>>,
}

impl Shell for SlowStartShell {
    fn run(&self, program: &str, args: &[String]) -> Result<Output> {
        assert_eq!(program, "herdr");
        self.calls.lock().unwrap().push(args.join(" "));
        let reply = |value: Value| {
            Ok(Output {
                status: 0,
                stdout: value.to_string(),
                stderr: String::new(),
            })
        };
        match args
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .as_slice()
        {
            ["pane", "list"] => reply(json!({"result":{"panes":[{
                "pane_id":"w2:p2","terminal_id":"term_shell"
            }]}})),
            ["agent", "list"] => {
                let seen = self.lists.fetch_add(1, Ordering::SeqCst);
                let agents = if seen >= self.ready_after {
                    vec![self.agent.clone()]
                } else {
                    vec![]
                };
                reply(json!({"result":{"agents":agents}}))
            }
            ["agent", "start", ..] => Ok(Output {
                status: 1,
                stdout: String::new(),
                stderr: r#"{"error":{"code":"timeout","message":"timed out waiting for agent startup"}}"#
                    .into(),
            }),
            ["agent", "rename", "w2:p2", "worker"] => reply(json!({"result":{}})),
            ["pane", "read", ..] => Err(Error::Invalid("startup output unavailable".into())),
            _ => panic!("unexpected command {args:?}"),
        }
    }
}

fn slow_start(ready_after: usize, agent: Value) -> SlowStartShell {
    SlowStartShell {
        lists: AtomicUsize::new(0),
        ready_after,
        agent,
        calls: std::sync::Mutex::default(),
    }
}

fn unnamed(session: &str) -> Value {
    json!({"pane_id":"w2:p2","terminal_id":"term_shell","agent":"codex","name":null,
        "agent_session":{"kind":"path","value":session}})
}

/// A start that Herdr gave up waiting for is not lost: its agent, found unnamed in the launch's
/// own terminal, gets the launch's name and the launch is confirmed. Without that name the
/// worker would show as a lead of its own, outside its owner's Workers tab and its close.
#[test]
fn a_timed_out_start_adopts_its_unnamed_agent_and_names_it() {
    let ledger = Ledger::new(std::env::temp_dir().join(crate::ledger::new_id()));
    let progress: Progress = serde_json::from_value(json!({
        "request":{"id":"slow","validity":validity_for(&ledger,"slow"),"action":{"type":"agent","kind":"codex","name":"worker","message":"go"},
            "host":"peer","cwd":"/","pane":"w2:p2","terminal":"term_shell","owner":"parent"},
        "pane":"w2:p2","terminal":"term_shell","attempted":false,"confirmed":false,
        "result":{"host":"peer","pane":"w2:p2"},"error":null
    })).unwrap();
    crate::launch_store::put_progress_value(&ledger, "slow", &progress).unwrap();
    // The agent shows on the third look.
    let shell = slow_start(2, unnamed("/sessions/one.jsonl"));
    let mut progress = crate::launch_store::progress(&ledger, "slow")
        .unwrap()
        .unwrap();
    let pauses = std::cell::Cell::new(0);
    let wait = Adoption {
        window: std::time::Duration::from_secs(60),
        elapsed: &|| std::time::Duration::from_secs(pauses.get() * 2),
        pause: &|_| pauses.set(pauses.get() + 1),
    };
    assert!(
        adopt(
            &ledger,
            &shell,
            &progress.request,
            "w2:p2",
            "term_shell",
            &wait
        )
        .unwrap()
    );
    assert_eq!(pauses.get(), 2, "it looks again until the agent shows");
    assert!(
        shell
            .calls
            .lock()
            .unwrap()
            .contains(&"agent rename w2:p2 worker".to_owned())
    );
    // Through the launch: the first list is the check before the start, and the agent
    // shows on the first look after the timeout.
    let shell = slow_start(1, unnamed("/sessions/one.jsonl"));
    let result = start_or_reconcile(&ledger, &shell, &mut progress, "w2:p2", "slow");
    assert!(result.is_ok(), "{result:?}");
    let calls = shell.calls.lock().unwrap().clone();
    assert_eq!(
        calls
            .iter()
            .filter(|call| call.starts_with("agent start"))
            .count(),
        1
    );
    assert!(
        calls.iter().any(|call| call
            .starts_with("agent start worker --kind codex --pane w2:p2 --timeout 120000 --")),
        "{calls:?}"
    );
    assert!(calls.contains(&"agent rename w2:p2 worker".to_owned()));
    // A replay that finds the unnamed agent names it without a second start.
    let shell = slow_start(0, unnamed("/sessions/one.jsonl"));
    assert!(start_or_reconcile(&ledger, &shell, &mut progress, "w2:p2", "slow").is_ok());
    assert!(
        !shell
            .calls
            .lock()
            .unwrap()
            .iter()
            .any(|call| call.starts_with("agent start"))
    );
    std::fs::remove_dir_all(ledger.dir).unwrap();
}

#[test]
fn a_timed_out_start_never_takes_a_named_or_foreign_agent() {
    let ledger = Ledger::new(std::env::temp_dir().join(crate::ledger::new_id()));
    let request: Launch = serde_json::from_value(json!({
        "id":"slow","validity":validity_for(&ledger,"slow"),"action":{"type":"agent","kind":"codex","name":"worker","message":"go"},
        "host":"peer","cwd":"/","pane":"w2:p2","terminal":"term_shell","owner":"parent"
    }))
    .unwrap();
    let wait = Adoption {
        window: std::time::Duration::ZERO,
        elapsed: &|| std::time::Duration::ZERO,
        pause: &|_| panic!("a zero window looks once"),
    };
    let mut named = unnamed("/s");
    named["name"] = json!("other");
    let mut foreign = unnamed("/s");
    foreign["terminal_id"] = json!("term_other");
    for agent in [named, foreign] {
        let shell = slow_start(0, agent);
        assert!(!adopt(&ledger, &shell, &request, "w2:p2", "term_shell", &wait).unwrap());
        assert!(
            !shell
                .calls
                .lock()
                .unwrap()
                .iter()
                .any(|call| call.starts_with("agent rename"))
        );
    }
    // A unit launch adopts only the agent whose session is in its fresh session directory.
    let sessions = std::path::Path::new("/state/launches/slow.sessions");
    let probe = |session: Option<&str>| crate::probe::Agent {
        pane: "w2:p2".into(),
        agent: Some("pi".into()),
        name: None,
        agent_session: session.map(Into::into),
        ..crate::probe::Agent::default()
    };
    assert!(unnamed_start(
        &probe(Some("/state/launches/slow.sessions/1_slow.jsonl")),
        Some(sessions)
    ));
    assert!(!unnamed_start(
        &probe(Some("/home/.pi/sessions/1.jsonl")),
        Some(sessions)
    ));
    assert!(!unnamed_start(&probe(None), Some(sessions)));
    assert!(unnamed_start(&probe(None), None));
    std::fs::remove_dir_all(ledger.dir).unwrap();
}
