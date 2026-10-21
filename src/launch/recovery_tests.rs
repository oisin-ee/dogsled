use super::*;
use crate::shell::Output;
use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Debug)]
struct LockedShell {
    ledger: Ledger,
    closed: AtomicUsize,
}

impl Shell for LockedShell {
    fn run(&self, program: &str, args: &[String]) -> Result<Output> {
        assert_eq!(program, "herdr");
        let receipt = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(self.ledger.child("launches").dir.join("own.lock"))?;
        assert!(matches!(
            receipt.try_lock(),
            Err(std::fs::TryLockError::WouldBlock)
        ));
        let panes = self.ledger.child("launch-panes");
        let file = std::fs::read_dir(&panes.dir)?.next().unwrap()?.path();
        let pane = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(file)?;
        assert!(matches!(
            pane.try_lock(),
            Err(std::fs::TryLockError::WouldBlock)
        ));
        let value = match args
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .as_slice()
        {
            ["pane", "list"] => json!({"result":{"panes":[{
                "pane_id":"w2:p2","terminal_id":"term_shell","cwd":"/checkout"
            }]}}),
            ["agent", "list"] => json!({"result":{"agents":[]}}),
            ["pane", "process-info", "--pane", "w2:p2"] => json!({"result":{"process_info":{
                "shell_pid":10,"foreground_process_group_id":10,
                "foreground_processes":[{"pid":10,"name":"zsh"}]
            }}}),
            ["pane", "close", "w2:p2"] => {
                self.closed.fetch_add(1, Ordering::SeqCst);
                json!({})
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

fn fixture() -> Ledger {
    let ledger = Ledger::new(std::env::temp_dir().join(crate::ledger::new_id()));
    ledger.child("launches").write("own.progress.json", &json!({
        "request":{"id":"own","action":{"type":"agent","kind":"pi","name":"worker","message":"go"},
            "host":"local","cwd":"/checkout","pane":"w2:p2","terminal":"term_shell","owner":"w1:p1"},
        "pane":"w2:p2","terminal":"term_shell","attempted":true,"confirmed":false,
        "result":{},"error":"response lost"
    })).unwrap();
    ledger
}

#[test]
fn receipt_and_pane_locks_span_every_verification_and_native_close() {
    let ledger = fixture();
    let shell = LockedShell {
        ledger: ledger.clone(),
        closed: AtomicUsize::new(0),
    };
    assert!(close_unfinished(&ledger, &shell, "w2:p2").unwrap());
    assert_eq!(shell.closed.load(Ordering::SeqCst), 1);
    std::fs::remove_dir_all(ledger.dir).unwrap();
}

#[test]
fn final_unattempted_and_refused_shells_close_under_both_locks_without_changing_evidence() {
    for refused in [false, true] {
        let ledger = fixture();
        let receipts = ledger.child("launches");
        let mut progress = receipts
            .read::<Progress>("own.progress.json")
            .unwrap()
            .unwrap();
        progress.attempted = false;
        receipts.write("own.progress.json", &progress).unwrap();
        if refused {
            receipts
                .write(
                    "own.json",
                    &Receipt::Refused {
                        failed: "agent_pane_busy".into(),
                    },
                )
                .unwrap();
        }
        let before = std::fs::read(receipts.dir.join("own.progress.json")).unwrap();
        let receipt = std::fs::read(receipts.dir.join("own.json")).ok();
        let shell = LockedShell {
            ledger: ledger.clone(),
            closed: AtomicUsize::new(0),
        };
        assert!(close_unfinished(&ledger, &shell, "w2:p2").unwrap());
        assert_eq!(shell.closed.load(Ordering::SeqCst), 1);
        assert_eq!(
            std::fs::read(receipts.dir.join("own.progress.json")).unwrap(),
            before
        );
        assert_eq!(std::fs::read(receipts.dir.join("own.json")).ok(), receipt);
        std::fs::remove_dir_all(ledger.dir).unwrap();
    }
}

#[test]
fn pending_paths_require_an_allocating_launch_or_its_matching_attempted_agent() {
    let ledger = fixture();
    let receipts = ledger.child("launches");
    let mut progress = receipts
        .read::<Progress>("own.progress.json")
        .unwrap()
        .unwrap();
    let matching = crate::probe::Agent {
        pane: "w2:p2".into(),
        name: Some("worker".into()),
        agent: Some("pi".into()),
        terminal_id: Some("term_shell".into()),
        ..Default::default()
    };
    assert_eq!(
        crate::launch::pending_paths(&ledger, &[]).unwrap(),
        Vec::<String>::new()
    );
    assert_eq!(
        crate::launch::pending_paths(&ledger, std::slice::from_ref(&matching)).unwrap(),
        ["/checkout"]
    );
    for field in ["pane", "name", "kind", "terminal"] {
        let mut changed = matching.clone();
        match field {
            "pane" => changed.pane = "w3:p1".into(),
            "name" => changed.name = Some("foreign".into()),
            "kind" => changed.agent = Some("claude".into()),
            _ => changed.terminal_id = Some("replacement".into()),
        }
        assert_eq!(
            crate::launch::pending_paths(&ledger, &[changed]).unwrap(),
            Vec::<String>::new()
        );
    }
    progress.attempted = false;
    receipts.write("own.progress.json", &progress).unwrap();
    assert_eq!(
        crate::launch::pending_paths(&ledger, &[matching]).unwrap(),
        Vec::<String>::new()
    );
    progress.pane = None;
    receipts.write("own.progress.json", &progress).unwrap();
    assert_eq!(
        crate::launch::pending_paths(&ledger, &[]).unwrap(),
        ["/checkout"]
    );
    receipts
        .write(
            "own.json",
            &Receipt::Refused {
                failed: "agent_pane_busy".into(),
            },
        )
        .unwrap();
    assert_eq!(
        crate::launch::pending_paths(&ledger, &[]).unwrap(),
        Vec::<String>::new()
    );
    std::fs::remove_dir_all(ledger.dir).unwrap();
}

#[test]
fn occupied_receipt_lock_prevents_all_terminal_operations() {
    let mut ledger = fixture();
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(ledger.child("launches").dir.join("own.lock"))
        .unwrap();
    lock.lock().unwrap();
    ledger.budget = crate::budget::Budget::until(
        std::time::Instant::now() + std::time::Duration::from_millis(30),
    );
    let shell = LockedShell {
        ledger: ledger.clone(),
        closed: AtomicUsize::new(0),
    };
    assert!(matches!(
        close_unfinished(&ledger, &shell, "w2:p2"),
        Err(Error::Deadline(_))
    ));
    assert_eq!(shell.closed.load(Ordering::SeqCst), 0);
    drop(lock);
    ledger.budget = crate::budget::Budget::default();
    assert!(close_unfinished(&ledger, &shell, "w2:p2").unwrap());
    assert_eq!(shell.closed.load(Ordering::SeqCst), 1);
    std::fs::remove_dir_all(ledger.dir).unwrap();
}

#[test]
fn recovery_rereads_a_changed_candidate_after_the_request_lock() {
    for change in ["confirmed", "receipt", "terminal"] {
        let ledger = fixture();
        let receipts = ledger.child("launches");
        let original = receipts
            .read::<Progress>("own.progress.json")
            .unwrap()
            .unwrap();
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(receipts.dir.join("own.lock"))
            .unwrap();
        lock.lock().unwrap();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let worker_ledger = ledger.clone();
        let worker = std::thread::spawn(move || {
            let shell = LockedShell {
                ledger: worker_ledger.clone(),
                closed: AtomicUsize::new(0),
            };
            ready_tx.send(()).unwrap();
            let result = close_selected(&worker_ledger, &shell, "w2:p2", "own", &original);
            assert_eq!(shell.closed.load(Ordering::SeqCst), 0);
            result
        });
        ready_rx.recv().unwrap();
        let mut progress = receipts
            .read::<Progress>("own.progress.json")
            .unwrap()
            .unwrap();
        match change {
            "confirmed" => progress.confirmed = true,
            "terminal" => progress.terminal = Some("replacement".into()),
            "receipt" => receipts
                .write("own.json", &Receipt::Started(json!({"pane":"w2:p2"})))
                .unwrap(),
            _ => unreachable!(),
        }
        receipts.write("own.progress.json", &progress).unwrap();
        drop(lock);
        assert!(matches!(worker.join().unwrap(), Err(Error::Invalid(_))));
        std::fs::remove_dir_all(ledger.dir).unwrap();
    }
}
