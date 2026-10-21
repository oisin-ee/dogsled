use super::*;
use crate::shell::Output;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Debug)]
struct StartingShell {
    starts: AtomicUsize,
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
        "request":{"id":"different","action":{"type":"agent","kind":"pi","name":"worker","message":"go"},
            "host":"local","cwd":"/checkout","pane":"w2:p2","terminal":"term_shell","owner":"w1:p1"},
        "pane":"w2:p2","terminal":"term_shell","attempted":false,"confirmed":false,
        "result":{},"error":null
    })).unwrap();
    ledger
        .child("launches")
        .write("different.progress.json", &progress)
        .unwrap();
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
    assert!(
        !ledger
            .child("launches")
            .read::<Progress>("different.progress.json")
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
    let unconfirmed = ledger
        .child("launches")
        .read::<Progress>("different.progress.json")
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
            "request":{"id":"stamp","action":{"type":"agent","kind":"codex","name":"worker","message":"go"},
                "host":"peer","cwd":"/","pane":"w2:p2","terminal":"term_shell","owner":"parent"},
            "pane":"w2:p2","terminal":"term_shell","attempted":recovering,"confirmed":false,
            "result":{"host":"peer","pane":"w2:p2"},"error":null
        })).unwrap();
        ledger
            .child("launches")
            .write("stamp.progress.json", &progress)
            .unwrap();
        let shell = ConfirmingShell {
            recovering,
            starts: AtomicUsize::new(0),
        };
        let before = crate::ledger::now_ms();
        // The caller's old admission timestamp must not become confirmation time.
        let result = work::launch(&ledger, &shell, &progress.request, 1).unwrap();
        let confirmed = result["launch_confirmed_ms"].as_u64().unwrap();
        assert!(confirmed >= before && confirmed <= crate::ledger::now_ms());
        assert_eq!(
            shell.starts.load(Ordering::SeqCst),
            usize::from(!recovering)
        );
        let saved = ledger
            .child("launches")
            .read::<Progress>("stamp.progress.json")
            .unwrap()
            .unwrap();
        assert!(saved.confirmed);
        assert_eq!(saved.result, result);
        // Simulate interruption between confirmation and final receipt publication.
        std::fs::remove_file(ledger.child("launches").dir.join("stamp.json")).unwrap();
        assert_eq!(agent(&ledger, &shell, &progress.request).unwrap(), result);
        assert_eq!(agent(&ledger, &shell, &progress.request).unwrap(), result);
        assert_eq!(
            shell.starts.load(Ordering::SeqCst),
            usize::from(!recovering)
        );
        std::fs::remove_dir_all(ledger.dir).unwrap();
    }
}
