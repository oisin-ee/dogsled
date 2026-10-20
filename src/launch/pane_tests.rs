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
    std::fs::remove_dir_all(ledger.dir).unwrap();
}
