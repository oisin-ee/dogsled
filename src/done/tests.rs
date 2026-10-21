use super::*;
use crate::shell::Fake;
use serde_json::json;

fn ledger() -> Ledger {
    Ledger::new(std::env::temp_dir().join(crate::ledger::new_id()))
}

fn agent(state: &str) -> Agent {
    Agent {
        pane: "w2:p1".into(),
        state: state.into(),
        agent: Some("pi".into()),
        agent_session: Some("session".into()),
        terminal_id: Some("old".into()),
        state_change_seq: Some(10),
        ..Agent::default()
    }
}

fn mark_at() -> Mark {
    Mark {
        pane: "w2:p1".into(),
        item: Some("launch".into()),
        session: Some("session".into()),
        name: Some("worker".into()),
        after: Some(10),
        sealed: None,
        marked_ms: 1,
        branch: None,
        reply: None,
        closed_ms: None,
    }
}

fn put(ledger: &Ledger, mark: Mark) {
    ledger
        .write(FILE, &Marks::from([(key(&mark), mark)]))
        .unwrap();
}

#[test]
fn restored_terminal_without_counters_seals_the_same_session() {
    let ledger = ledger();
    put(&ledger, mark_at());
    let mut restored = agent("idle");
    restored.terminal_id = Some("new".into());
    restored.state_change_seq = None;
    assert!(
        finished(&ledger, &[restored.clone()])
            .unwrap()
            .contains("w2:p1")
    );
    assert_eq!(list(&ledger).unwrap()[0].sealed, Some(0));
    restored.state = "working".into();
    assert!(finished(&ledger, &[restored.clone()]).unwrap().is_empty());
    assert_eq!(list(&ledger).unwrap()[0].sealed, Some(0));
    restored.state = "idle".into();
    restored.agent_session = Some("other".into());
    assert!(finished(&ledger, &[restored]).unwrap().is_empty());
}

#[test]
fn idle_without_done_and_orchestrators_stay_open() {
    let ledger = ledger();
    assert!(finished(&ledger, &[agent("idle")]).unwrap().is_empty());
    put(&ledger, mark_at());
    crate::items::record(
        &ledger,
        crate::items::Item {
            id: "child".into(),
            kind: "agent".into(),
            host: "local".into(),
            pane: "w3:p1".into(),
            owner_pane: "w2:p1".into(),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(finished(&ledger, &[agent("idle")]).unwrap().is_empty());
}

#[test]
fn inbox_cancels_only_during_the_done_turn_and_never_after_it() {
    let ledger = ledger();
    put(&ledger, mark_at());
    input(
        &ledger,
        &agent("working"),
        Input::Put { from: None },
        || Ok(((), true)),
    )
    .unwrap();
    assert_eq!(list(&ledger).unwrap(), []);
    put(&ledger, mark_at());
    let refused = input(
        &ledger,
        &agent("idle"),
        Input::Put { from: None },
        || -> Result<((), bool)> { panic!("must not deliver") },
    );
    assert!(
        refused
            .unwrap_err()
            .to_string()
            .contains("worker finished; start a new child for more work")
    );
    assert!(
        input(
            &ledger,
            &agent("working"),
            Input::Put { from: None },
            || Ok(((), true))
        )
        .is_err()
    );
    assert_eq!(list(&ledger).unwrap().len(), 1);
}

#[test]
fn an_overlapping_branch_refresh_cannot_erase_a_terminal_seal() {
    let ledger = ledger();
    let older = mark_at();
    put(&ledger, older.clone());
    finished(&ledger, &[agent("idle")]).unwrap();
    save(&ledger, &older).unwrap();
    assert_eq!(list(&ledger).unwrap()[0].sealed, Some(0));
    assert!(
        input(
            &ledger,
            &agent("working"),
            Input::Put { from: None },
            || Ok(((), true))
        )
        .is_err()
    );
}

#[test]
fn legacy_migration_retains_missing_marks_and_rekeys_restored_panes_once() {
    let ledger = ledger();
    ledger
        .write(
            FILE,
            &json!({
                "old_terminal":{"pane":"w2:p1","after":10,"sealed":12,"marked_ms":1},
                "missing_terminal":{"pane":"w9:p1","after":20,"marked_ms":2}
            }),
        )
        .unwrap();
    let mut restored = agent("idle");
    restored.terminal_id = Some("new".into());
    assert!(
        finished(&ledger, &[restored.clone()])
            .unwrap()
            .contains("w2:p1")
    );
    let first: Marks = ledger.read(FILE).unwrap().unwrap();
    assert_eq!(first.len(), 2);
    assert!(!first.contains_key("old_terminal"));
    assert!(first.contains_key("missing_terminal"));
    finished(&ledger, &[restored]).unwrap();
    assert_eq!(first, ledger.read(FILE).unwrap().unwrap());
}

#[test]
fn done_refuses_every_nonworking_state_without_recording() {
    for state in ["idle", "done", "blocked", "unknown"] {
        let ledger = ledger();
        let shell = Fake::new();
        shell.on(
            "herdr agent list",
            &json!({"result":{"agents":[{
                "pane_id":"w2:p1","agent":"pi","agent_status":state,
                "agent_session":{"kind":"path","value":"session"}
            }]}})
            .to_string(),
        );
        assert!(
            mark(&ledger, &shell, "w2:p1", 1)
                .unwrap_err()
                .to_string()
                .contains("call done during the turn")
        );
        assert!(!ledger.dir.join(FILE).exists());
    }
}

#[test]
fn done_refuses_open_children_and_pending_or_admitting_tickets() {
    for queued in [None, Some("pending"), Some("admitting")] {
        let ledger = ledger();
        let shell = Fake::new();
        shell.on("herdr agent list", r#"{"result":{"agents":[{"pane_id":"w2:p1","agent":"pi","agent_status":"working","agent_session":{"kind":"path","value":"session"}}]}}"#);
        if let Some(state) = queued {
            ledger.write("queue.json", &json!([{
                "id":"q","state":state,"created_ms":1,"waiting":false,"waiting_until_ms":null,"notified":false,"result":null,"error":null,
                "request":{"id":"q","action":{"type":"place"},"host":null,"branch":null,"base":null,"cwd":"/tmp","repository":null,"workspace":null,"workspace_name":null,"owner":"w2:p1"}
            }])).unwrap();
        } else {
            crate::items::record(
                &ledger,
                crate::items::Item {
                    id: "child".into(),
                    owner_pane: "w2:p1".into(),
                    ..Default::default()
                },
            )
            .unwrap();
        }
        assert!(
            mark(&ledger, &shell, "w2:p1", 1)
                .unwrap_err()
                .to_string()
                .contains("still owns open work")
        );
        assert!(!ledger.dir.join(FILE).exists());
    }
}
