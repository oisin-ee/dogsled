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
fn survey_retains_one_launch_snapshot_but_recovery_rereads_before_closing() {
    let ledger = fixture();
    let panes = vec![crate::probe::Pane {
        pane_id: "w2:p2".into(),
        cwd: Some("/checkout".into()),
        ..Default::default()
    }];
    let terminals = std::collections::BTreeMap::from([("w2:p2".into(), "term_shell".into())]);
    let survey = Survey::load(&ledger, &panes, &terminals, &[]).unwrap();
    assert_eq!(survey.pane("w2:p2"), PaneLaunch::Final);
    assert_eq!(survey.pane("unrecorded"), PaneLaunch::Unrecorded);
    ledger
        .child("launches")
        .write("own.json", &Receipt::Started(json!({"pane":"w2:p2"})))
        .unwrap();
    assert_eq!(survey.pane("w2:p2"), PaneLaunch::Final);
    let shell = LockedShell {
        ledger: ledger.clone(),
        closed: AtomicUsize::new(0),
    };
    assert!(!close_unfinished(&ledger, &shell, "w2:p2").unwrap());
    assert_eq!(shell.closed.load(Ordering::SeqCst), 0);
    std::fs::remove_dir_all(ledger.dir).unwrap();
}

#[derive(Debug)]
struct CurrentShell;

impl Shell for CurrentShell {
    fn run(&self, program: &str, args: &[String]) -> Result<Output> {
        assert_eq!(program, "herdr");
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
            _ => panic!("unexpected command {args:?}"),
        };
        Ok(Output {
            status: 0,
            stdout: value.to_string(),
            stderr: String::new(),
        })
    }
}

fn assert_pane_state(ledger: &Ledger, expected: PaneLaunch) {
    let panes = crate::cleanup::current_panes(&CurrentShell).unwrap();
    let terminals = std::collections::BTreeMap::from([("w2:p2".into(), "term_shell".into())]);
    let survey = Survey::load(ledger, &panes, &terminals, &[]).unwrap();
    assert_eq!(survey.pane("w2:p2"), expected);
    assert_eq!(
        final_pane(ledger, &CurrentShell, "w2:p2").unwrap(),
        expected
    );
}

#[test]
fn changed_terminals_release_non_started_records_but_matching_and_started_records_stay() {
    for state in ["refused", "unconfirmed", "confirmed", "started"] {
        for changed in [false, true] {
            let ledger = fixture();
            let receipts = ledger.child("launches");
            let mut progress = receipts
                .read::<Progress>("own.progress.json")
                .unwrap()
                .unwrap();
            progress.confirmed = state == "confirmed";
            if changed {
                progress.terminal = Some("term_before_restore".into());
            }
            receipts.write("own.progress.json", &progress).unwrap();
            match state {
                "refused" => receipts
                    .write(
                        "own.json",
                        &Receipt::Refused {
                            failed: "agent_pane_busy".into(),
                        },
                    )
                    .unwrap(),
                "started" => receipts
                    .write("own.json", &Receipt::Started(json!({"pane":"w2:p2"})))
                    .unwrap(),
                _ => (),
            }
            let before = std::fs::read(receipts.dir.join("own.progress.json")).unwrap();
            let receipt = std::fs::read(receipts.dir.join("own.json")).ok();
            let expected = if state == "started" {
                PaneLaunch::Started
            } else if changed {
                PaneLaunch::Unrecorded
            } else if state == "confirmed" {
                PaneLaunch::Unresolved
            } else {
                PaneLaunch::Final
            };
            assert_pane_state(&ledger, expected);
            assert_eq!(
                std::fs::read(receipts.dir.join("own.progress.json")).unwrap(),
                before
            );
            assert_eq!(std::fs::read(receipts.dir.join("own.json")).ok(), receipt);
            std::fs::remove_dir_all(ledger.dir).unwrap();
        }
    }
}

#[test]
fn obsolete_records_do_not_compete_but_multiple_remaining_owners_still_hold() {
    for matching in [0, 1, 2] {
        for started in [false, true] {
            let ledger = fixture();
            let receipts = ledger.child("launches");
            let mut progress = receipts
                .read::<Progress>("own.progress.json")
                .unwrap()
                .unwrap();
            progress.terminal = Some("term_obsolete".into());
            receipts.write("own.progress.json", &progress).unwrap();
            if started {
                receipts
                    .write("own.json", &Receipt::Started(json!({"pane":"w2:p2"})))
                    .unwrap();
            }
            progress.request.id = "obsolete".into();
            receipts.write("obsolete.progress.json", &progress).unwrap();
            for index in 0..matching {
                let id = format!("matching-{index}");
                progress.request.id = id.clone();
                progress.terminal = Some("term_shell".into());
                receipts
                    .write(&format!("{id}.progress.json"), &progress)
                    .unwrap();
            }
            let expected = match (matching, started) {
                (0, false) => PaneLaunch::Unrecorded,
                (0, true) => PaneLaunch::Started,
                (1, false) => PaneLaunch::Final,
                _ => PaneLaunch::Unresolved,
            };
            assert_pane_state(&ledger, expected);
            // Filtering classification never weakens recovery's locked competing-record check.
            assert!(close_unfinished(&ledger, &CurrentShell, "w2:p2").is_err());
            std::fs::remove_dir_all(ledger.dir).unwrap();
        }
    }
}

#[test]
fn missing_terminal_identity_is_not_proof_that_a_record_has_lost_ownership() {
    for saved in [None, Some("")] {
        let ledger = fixture();
        let receipts = ledger.child("launches");
        let mut progress = receipts
            .read::<Progress>("own.progress.json")
            .unwrap()
            .unwrap();
        progress.terminal = saved.map(str::to_owned);
        receipts.write("own.progress.json", &progress).unwrap();
        assert_pane_state(&ledger, PaneLaunch::Unresolved);
        std::fs::remove_dir_all(ledger.dir).unwrap();
    }
}

fn pending_paths(ledger: &Ledger, agents: &[crate::probe::Agent]) -> Result<Vec<String>> {
    Ok(Survey::load(ledger, &[], &std::collections::BTreeMap::new(), agents)?.pending_paths)
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
    assert_eq!(pending_paths(&ledger, &[]).unwrap(), Vec::<String>::new());
    assert_eq!(
        pending_paths(&ledger, std::slice::from_ref(&matching)).unwrap(),
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
            pending_paths(&ledger, &[changed]).unwrap(),
            Vec::<String>::new()
        );
    }
    progress.attempted = false;
    receipts.write("own.progress.json", &progress).unwrap();
    assert_eq!(
        pending_paths(&ledger, &[matching]).unwrap(),
        Vec::<String>::new()
    );
    progress.pane = None;
    receipts.write("own.progress.json", &progress).unwrap();
    assert_eq!(pending_paths(&ledger, &[]).unwrap(), ["/checkout"]);
    receipts
        .write(
            "own.json",
            &Receipt::Refused {
                failed: "agent_pane_busy".into(),
            },
        )
        .unwrap();
    assert_eq!(pending_paths(&ledger, &[]).unwrap(), Vec::<String>::new());
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
