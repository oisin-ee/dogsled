use super::{
    BTreeMap, BTreeSet, Gate, Ledger, consume_quota_request, consume_sweep_request, custodies,
    gate, landing, marks, operations, orphans, parents, peer, put_custodies, put_landing,
    put_marks, put_operations, put_orphans, put_parents, put_peer, put_quota, put_quota_refresh,
    put_quota_request, put_report, put_repositories, put_snapshot, put_sweep_request, put_trees,
    put_watchdog, quota, quota_refresh, quota_request, replace_gate, report, repositories,
    snapshot, sweep_request, trees, watchdog,
};
use serde_json::{Value, json};

fn ledger() -> Ledger {
    Ledger::new(std::env::temp_dir().join(crate::ledger::new_id()))
}
fn clear_lists(value: &mut Value) {
    match value {
        Value::Array(values) => values.clear(),
        Value::Object(fields) => {
            for (name, value) in fields {
                if [
                    "sent",
                    "events",
                    "marks",
                    "notified",
                    "stopped",
                    "prompted_after",
                    "by_status",
                ]
                .contains(&name.as_str())
                {
                    *value = json!({});
                } else {
                    clear_lists(value);
                }
            }
        }
        _ => {}
    }
}
fn populate(ledger: &Ledger) {
    let snapshot_value: crate::node::Snapshot = serde_json::from_str(r#"{"sampled_ms":7,"probe":{"host":"value","label":"value","load1":0.25,"cores":7,"cpu_ticks":{"busy":7,"total":7},"busy":0.25,"memory":{"used_bytes":7,"total_bytes":7,"pressure_level":7},"disk_free_percent":0.25,"agents":{"total":7,"by_status":{"key":7}},"agent_panes":[{"pane":"value","agent":"value","state":"value","name":"value","transcript":"value","transcript_modified":7,"stalled_since_ms":7,"state_since_ms":7,"signal":"value","terminal_id":"value","agent_session":"value","completion_seq":7,"state_change_seq":7,"cwd":"value","session":{"model":"value","response_model":"value","thinking":"value","activity":"value"},"worktree":{"path":"value","branch":"value","ahead":7,"dirty":true,"landed":true,"head":"value","base":"value"},"repository":"value","done":true,"finishing":true,"handoff":"value","launch_progress":"value","owns_open":true,"lifecycle":"value","lifecycle_gap":"value"}],"reservations":[{"id":"value","created_ms":7}],"panes":[{"pane_id":"value","focused":true,"workspace_id":"value","name":"value","agent":"value","agent_status":"value","cwd":"value","foreground_cwd":"value","idle_shell":true,"idle_since_ms":7}]},"jobs":[{"id":"value","cmd":["value"],"cwd":"value","host":"value","owner":"value","pane":"value","state":"value","exit":7,"signal":7,"reason":"value","started_ms":7,"ended_ms":7,"timeout_ms":7,"idle_timeout_ms":7,"validity":{"version":7,"created_ms":7},"startup":{"version":1,"scope":"non_repository","cwd":"/tmp/value","preparation":null},"output_expired":true}]}"#).unwrap();
    put_snapshot(ledger, &snapshot_value).unwrap();
    assert_eq!(
        serde_json::to_value(snapshot(ledger).unwrap().unwrap()).unwrap(),
        serde_json::to_value(&snapshot_value).unwrap()
    );
    let watchdog_value: crate::watchdog::State = serde_json::from_str(r#"{"last_scan_ms":7,"sent":{"key":7},"events":{"key":{"signature":"value","last_ms":7,"urgent":true}},"marks":{"key":"value"},"notified":{"key":7},"stopped":{"key":{"pane":"value","agent_session":"value","turn":{"state_change_seq":7,"state_since_ms":7},"message_id":"value","attempts":7,"delivered":true,"prompted_after":7}},"prompted_after":{"key":{"prompted_after":7,"turn":{"state_change_seq":7,"state_since_ms":7},"working_turn":{"state_change_seq":7,"state_since_ms":7},"last_observed_working":true,"last_observed_ms":7}}}"#).unwrap();
    put_watchdog(ledger, &watchdog_value).unwrap();
    assert_eq!(
        serde_json::to_value(watchdog(ledger).unwrap().unwrap()).unwrap(),
        serde_json::to_value(&watchdog_value).unwrap()
    );
    let peer_value: crate::hosts::Cached = serde_json::from_str(r#"{"fetched_ms":7,"source":["value"],"snapshot":{"sampled_ms":7,"probe":{"host":"value","label":"value","load1":0.25,"cores":7,"cpu_ticks":{"busy":7,"total":7},"busy":0.25,"memory":{"used_bytes":7,"total_bytes":7,"pressure_level":7},"disk_free_percent":0.25,"agents":{"total":7,"by_status":{"key":7}},"agent_panes":[{"pane":"value","agent":"value","state":"value","name":"value","transcript":"value","transcript_modified":7,"stalled_since_ms":7,"state_since_ms":7,"signal":"value","terminal_id":"value","agent_session":"value","completion_seq":7,"state_change_seq":7,"cwd":"value","session":{"model":"value","response_model":"value","thinking":"value","activity":"value"},"worktree":{"path":"value","branch":"value","ahead":7,"dirty":true,"landed":true,"head":"value","base":"value"},"repository":"value","done":true,"finishing":true,"handoff":"value","launch_progress":"value","owns_open":true,"lifecycle":"value","lifecycle_gap":"value"}],"reservations":[{"id":"value","created_ms":7}],"panes":[{"pane_id":"value","focused":true,"workspace_id":"value","name":"value","agent":"value","agent_status":"value","cwd":"value","foreground_cwd":"value","idle_shell":true,"idle_since_ms":7}]},"jobs":[{"id":"value","cmd":["value"],"cwd":"value","host":"value","owner":"value","pane":"value","state":"value","exit":7,"signal":7,"reason":"value","started_ms":7,"ended_ms":7,"timeout_ms":7,"idle_timeout_ms":7,"validity":{"version":7,"created_ms":7},"startup":{"version":1,"scope":"non_repository","cwd":"/tmp/value","preparation":null},"output_expired":true}]},"reachable":true,"available":true,"running":true,"handoffs":[{"pane":"value","item":"value","session":"value","name":"value","after":7,"sealed":7,"marked_ms":7,"done_turn":{"state_change_seq":7,"state_since_ms":7},"signal":{"kind":"ask","text":"value","event_id":"value","turn":{"state_change_seq":7,"state_since_ms":7},"session":"value","recorded_ms":7,"cleared_ms":7},"branch":{"repo":"value","name":"value","ahead":7,"landed":true},"detached":true,"reply":{"seq":7,"result_id":"value","text":"value","captured_ms":7,"reported_ms":7,"done":true},"native_done":true,"closed_ms":7,"dropped_ms":7}],"holds":[{"host":"host","resource":"resource","owner":"owner","owner_pane":"pane","code":"code","reason":"reason","next_command":"dogsled status","command_host":"host","command_argv":["dogsled","status"],"request_id":"request"}],"error":"value","failure":"timeout","retry":{"attempts":7,"next_ms":7},"job_validity":7,"service_lanes":[{"lane":"admission","error":"value","since_ms":7,"last_ms":7,"repeats":7}]}"#).unwrap();
    put_peer(ledger, "peer", &peer_value).unwrap();
    assert_eq!(
        serde_json::to_value(peer(ledger, "peer").unwrap().unwrap()).unwrap(),
        serde_json::to_value(&peer_value).unwrap()
    );
    let quota_value: crate::quota::Snapshot = serde_json::from_str(r#"{"version":1,"fetched_ms":7,"source":"value","pools":[{"pool":"value","account":"value","window":"value","used_pct":0.25,"resets_ms":7,"error":"value"}]}"#).unwrap();
    put_quota(ledger, &quota_value).unwrap();
    assert_eq!(
        serde_json::to_value(quota(ledger).unwrap().unwrap()).unwrap(),
        serde_json::to_value(&quota_value).unwrap()
    );
    let quota_refresh_value: crate::quota::Refresh = serde_json::from_str(r#"{"attempted_ms":7,"attempts":7,"retry":{"attempts":7,"next_ms":7},"pending_push":true,"push_retry":{"attempts":7,"next_ms":7}}"#).unwrap();
    put_quota_refresh(ledger, &quota_refresh_value).unwrap();
    assert_eq!(
        serde_json::to_value(quota_refresh(ledger).unwrap().unwrap()).unwrap(),
        serde_json::to_value(&quota_refresh_value).unwrap()
    );
    let report_value: crate::sweep::Report = serde_json::from_str(r#"{"ts_ms":7,"started_ms":7,"finished_ms":7,"results":["value"],"failures":["value"],"skipped":["value"],"failure_entries":[{"entry":{"class":"value","id":"value","detail":"value","removable":true,"tree":{"repo":"value","path":"value","branch":"value","head":"value","identity":{"common":"/tmp/value","gitdir":"/tmp/value","generation":"value"}},"kind":"value","dirty_paths":["value"],"dirty_count":7},"detail":"value"}],"repositories":[{"class":"value","id":"value","detail":"value","removable":true,"tree":{"repo":"value","path":"value","branch":"value","head":"value","identity":{"common":"/tmp/value","gitdir":"/tmp/value","generation":"value"}},"kind":"value","dirty_paths":["value"],"dirty_count":7}],"landed_dirty":[{"class":"value","id":"value","detail":"value","removable":true,"tree":{"repo":"value","path":"value","branch":"value","head":"value","identity":{"common":"/tmp/value","gitdir":"/tmp/value","generation":"value"}},"kind":"value","dirty_paths":["value"],"dirty_count":7}],"unlinked":[{"class":"value","id":"value","detail":"value","removable":true,"tree":{"repo":"value","path":"value","branch":"value","head":"value","identity":{"common":"/tmp/value","gitdir":"/tmp/value","generation":"value"}},"kind":"value","dirty_paths":["value"],"dirty_count":7}]}"#).unwrap();
    put_report(ledger, &report_value).unwrap();
    assert_eq!(
        serde_json::to_value(report(ledger).unwrap().unwrap()).unwrap(),
        serde_json::to_value(&report_value).unwrap()
    );
    populate_collections(ledger);
}
fn populate_collections(ledger: &Ledger) {
    let marks_value: BTreeMap<String,crate::done::Mark> = serde_json::from_str(r#"{"ignored":{"pane":"value","item":"value","session":"value","name":"value","after":7,"sealed":7,"marked_ms":7,"done_turn":{"state_change_seq":7,"state_since_ms":7},"signal":{"kind":"ask","text":"value","event_id":"value","turn":{"state_change_seq":7,"state_since_ms":7},"session":"value","recorded_ms":7,"cleared_ms":7},"branch":{"repo":"value","name":"value","ahead":7,"landed":true},"detached":true,"reply":{"seq":7,"result_id":"value","text":"value","captured_ms":7,"reported_ms":7,"done":true},"native_done":true,"closed_ms":7,"dropped_ms":7}}"#).unwrap();
    let marks_value = marks_value
        .into_values()
        .map(|mark| (crate::done::key(&mark), mark))
        .collect();
    put_marks(ledger, &marks_value).unwrap();
    assert_eq!(
        serde_json::to_value(marks(ledger).unwrap()).unwrap(),
        serde_json::to_value(&marks_value).unwrap()
    );
    let trees_value: Vec<crate::cleanup::Tree> = serde_json::from_str(r#"[{"repo":"value","path":"value","branch":"value","head":"value","identity":{"common":"/tmp/value","gitdir":"/tmp/value","generation":"value"}}]"#).unwrap();
    put_trees(ledger, &trees_value).unwrap();
    assert_eq!(
        serde_json::to_value(trees(ledger).unwrap()).unwrap(),
        serde_json::to_value(&trees_value).unwrap()
    );
    let repositories_value: BTreeSet<String> = serde_json::from_str(r#"["/tmp/repo"]"#).unwrap();
    put_repositories(ledger, &repositories_value).unwrap();
    assert_eq!(
        serde_json::to_value(repositories(ledger).unwrap()).unwrap(),
        serde_json::to_value(&repositories_value).unwrap()
    );
    let custodies_value: Vec<crate::custody::Custody> = serde_json::from_str(r#"[{"item":"value","host":"value","owner_pane":"value","owner_session":"value","custodian_pane":"value","custodian_session":"value","revision":7,"claims":[{"id":"value","claimant_pane":"value","claimant_session":"value","claimed_ms":7,"previous_custodian":"value","revision":7}]}]"#).unwrap();
    put_custodies(ledger, &custodies_value).unwrap();
    assert_eq!(
        serde_json::to_value(custodies(ledger).unwrap()).unwrap(),
        serde_json::to_value(&custodies_value).unwrap()
    );
    let parents_value: BTreeMap<String, crate::watchdog::Parent> =
        serde_json::from_str(r#"{"pane":{"transcript":"value"}}"#).unwrap();
    put_parents(ledger, &parents_value).unwrap();
    assert_eq!(
        serde_json::to_value(parents(ledger).unwrap()).unwrap(),
        serde_json::to_value(&parents_value).unwrap()
    );
    let landing_value: BTreeMap<String, String> =
        serde_json::from_str(r#"{"item":"reason"}"#).unwrap();
    put_landing(ledger, &landing_value).unwrap();
    assert_eq!(
        serde_json::to_value(landing(ledger).unwrap()).unwrap(),
        serde_json::to_value(&landing_value).unwrap()
    );
    let orphans_value: Vec<crate::sweep::Orphan> = serde_json::from_str(r#"[{"item":"value","host":"value","pane":"value","owner_pane":"value","name":"value","reason":"value","since_ms":7,"status":"orphaned","claimed":true,"custodian_pane":"value"}]"#).unwrap();
    put_orphans(ledger, &orphans_value).unwrap();
    assert_eq!(
        serde_json::to_value(orphans(ledger).unwrap()).unwrap(),
        serde_json::to_value(&orphans_value).unwrap()
    );
    let operations_value: BTreeMap<String,crate::preservation::Operation> = serde_json::from_str(r#"{"operation":{"id":"value","expectation":{"pane":"value","terminal":"value","request":{"item":"item","created_ms":7,"owner_pane":"owner","owner_session":"session","host":"host"},"launch":"value","owners":[{"item":"item","created_ms":7,"owner_pane":"owner","owner_session":"session","host":"host"}],"tree":{"repo":"value","path":"value","branch":"value","head":"value","identity":{"common":"/tmp/value","gitdir":"/tmp/value","generation":"value"}},"mode":{"kind":"stopped_orphan","episode":{"pane":"pane","agent_session":"session","turn":{"state_change_seq":7,"state_since_ms":7}}},"caller":{"pane":"value","workspace":"value","cwd":"value"}},"initial_id":"value","successor_id":"value","initial":{"id":"snapshot","ref_name":"refs/dogsled/recovery/snapshot","commit":"commit","head":{"kind":"branch","name":"feature","commit":"head"},"index_tree":"index","worktree_tree":"working","raw_worktree_bytes":true,"created_ms":7,"ignored_files_captured":false,"context":null},"latest":{"id":"snapshot","ref_name":"refs/dogsled/recovery/snapshot","commit":"commit","head":{"kind":"branch","name":"feature","commit":"head"},"index_tree":"index","worktree_tree":"working","raw_worktree_bytes":true,"created_ms":7,"ignored_files_captured":false,"context":null},"stage":"intent","blocker":"value","result":{"pane":"pane","worktree":{"path":"/tmp/work","branch":"feature","workspace":"workspace","branch_deleted":true,"anchor_closed":"anchor"},"pane_closed":true,"retained_branch":{"repo":"value","name":"value","ahead":7,"landed":true},"recovery":{"host":"host","repository":"repo","id":"saved","pinned_oid":"oid","predecessors":["previous"],"operation":"operation","item":"item"}}}}"#).unwrap();
    put_operations(ledger, &operations_value).unwrap();
    assert_eq!(
        serde_json::to_value(operations(ledger).unwrap()).unwrap(),
        serde_json::to_value(&operations_value).unwrap()
    );
}
fn clear(ledger: &Ledger) {
    let snapshot_value = snapshot(ledger).unwrap().unwrap();
    let watchdog_value = watchdog(ledger).unwrap().unwrap();
    let peer_value = peer(ledger, "peer").unwrap().unwrap();
    let quota_value = quota(ledger).unwrap().unwrap();
    let quota_refresh_value = quota_refresh(ledger).unwrap().unwrap();
    let report_value = report(ledger).unwrap().unwrap();
    let mut next = serde_json::to_value(snapshot_value).unwrap();
    clear_lists(&mut next);
    let mut next: crate::node::Snapshot = serde_json::from_value(next).unwrap();
    next.probe.cpu_ticks = None;
    next.probe.busy = None;
    next.probe.memory.pressure_level = None;
    put_snapshot(ledger, &next).unwrap();
    assert_eq!(
        serde_json::to_value(snapshot(ledger).unwrap().unwrap()).unwrap(),
        serde_json::to_value(next).unwrap()
    );
    let mut next = serde_json::to_value(watchdog_value).unwrap();
    clear_lists(&mut next);
    let mut next: crate::watchdog::State = serde_json::from_value(next).unwrap();
    next.last_scan_ms = None;
    put_watchdog(ledger, &next).unwrap();
    assert_eq!(
        serde_json::to_value(watchdog(ledger).unwrap().unwrap()).unwrap(),
        serde_json::to_value(next).unwrap()
    );
    let mut next = serde_json::to_value(peer_value).unwrap();
    clear_lists(&mut next);
    let mut next: crate::hosts::Cached = serde_json::from_value(next).unwrap();
    next.snapshot = None;
    next.running = None;
    next.error = None;
    next.failure = None;
    next.job_validity = None;
    put_peer(ledger, "peer", &next).unwrap();
    assert_eq!(
        serde_json::to_value(peer(ledger, "peer").unwrap().unwrap()).unwrap(),
        serde_json::to_value(next).unwrap()
    );
    let mut next = serde_json::to_value(quota_value).unwrap();
    clear_lists(&mut next);
    let next: crate::quota::Snapshot = serde_json::from_value(next).unwrap();
    put_quota(ledger, &next).unwrap();
    assert_eq!(
        serde_json::to_value(quota(ledger).unwrap().unwrap()).unwrap(),
        serde_json::to_value(next).unwrap()
    );
    let mut next = serde_json::to_value(quota_refresh_value).unwrap();
    clear_lists(&mut next);
    let next: crate::quota::Refresh = serde_json::from_value(next).unwrap();
    put_quota_refresh(ledger, &next).unwrap();
    assert_eq!(
        serde_json::to_value(quota_refresh(ledger).unwrap().unwrap()).unwrap(),
        serde_json::to_value(next).unwrap()
    );
    let mut next = serde_json::to_value(report_value).unwrap();
    clear_lists(&mut next);
    let next: crate::sweep::Report = serde_json::from_value(next).unwrap();
    put_report(ledger, &next).unwrap();
    assert_eq!(
        serde_json::to_value(report(ledger).unwrap().unwrap()).unwrap(),
        serde_json::to_value(next).unwrap()
    );
    put_marks(ledger, &BTreeMap::new()).unwrap();
    put_trees(ledger, &Vec::new()).unwrap();
    put_repositories(ledger, &BTreeSet::new()).unwrap();
    put_custodies(ledger, &Vec::new()).unwrap();
    put_parents(ledger, &BTreeMap::new()).unwrap();
    put_landing(ledger, &BTreeMap::new()).unwrap();
    put_orphans(ledger, &Vec::new()).unwrap();
    put_operations(ledger, &BTreeMap::new()).unwrap();
    crate::store::write(ledger, |tx| replace_gate(tx, &Gate::new())).unwrap();
    consume_sweep_request(ledger, "token").unwrap();
    consume_quota_request(ledger, "token").unwrap();
}
#[test]
fn every_host_table_round_trips_and_replacement_clears_its_children() {
    let ledger = ledger();
    populate(&ledger);
    put_sweep_request(&ledger, "token").unwrap();
    assert_eq!(sweep_request(&ledger).unwrap().as_deref(), Some("token"));
    put_quota_request(&ledger, "token").unwrap();
    assert_eq!(quota_request(&ledger).unwrap().as_deref(), Some("token"));
    // Only the latest token is consumed: an older pass cannot drop a newer request.
    consume_quota_request(&ledger, "older").unwrap();
    assert_eq!(quota_request(&ledger).unwrap().as_deref(), Some("token"));
    let signatures = Gate::from([(
        "owner".into(),
        BTreeSet::from([BTreeSet::from([("item".into(), Some(7), true)])]),
    )]);
    crate::store::write(&ledger, |tx| replace_gate(tx, &signatures)).unwrap();
    assert_eq!(crate::store::read(&ledger, gate).unwrap(), signatures);
    let tables: Vec<String> = crate::store::read(&ledger,|tx|Ok(tx.prepare("SELECT name FROM sqlite_schema WHERE type='table' AND name LIKE 'host_%' ORDER BY name")?.query_map([],|row|row.get(0))?.collect::<rusqlite::Result<_>>()?)).unwrap();
    for table in &tables {
        let count: i64 = crate::store::read(&ledger, |tx| {
            Ok(
                tx.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                    row.get(0)
                })?,
            )
        })
        .unwrap();
        assert!(count > 0, "{table} must have a complete fixture");
        assert!(
            crate::store::write(&ledger, |tx| {
                let first_column: String = tx.query_row(
                    &format!("SELECT name FROM pragma_table_info('{table}') ORDER BY cid LIMIT 1"),
                    [],
                    |row| row.get(0),
                )?;
                tx.execute(
                    &format!("INSERT INTO {table} ({first_column}) VALUES ('')"),
                    [],
                )?;
                Ok(())
            })
            .is_err(),
            "{table} refuses malformed input"
        );
        assert_eq!(
            crate::store::read(&ledger, |tx| Ok(tx.query_row(
                &format!("SELECT count(*) FROM {table}"),
                [],
                |row| row.get::<_, i64>(0)
            )?))
            .unwrap(),
            count,
            "{table} stores nothing on failure"
        );
    }
    clear(&ledger);
    for table in tables {
        let children: i64 = crate::store::read(&ledger, |tx| {
            Ok(tx.query_row(
                &format!("SELECT count(*) FROM pragma_foreign_key_list('{table}')"),
                [],
                |row| row.get(0),
            )?)
        })
        .unwrap();
        if children > 0 {
            assert_eq!(
                crate::store::read(&ledger, |tx| Ok(tx.query_row(
                    &format!("SELECT count(*) FROM {table}"),
                    [],
                    |row| row.get::<_, i64>(0)
                )?))
                .unwrap(),
                0,
                "{table} clears child rows on update"
            );
        }
    }
    std::fs::remove_dir_all(ledger.dir).unwrap();
}

#[test]
fn malformed_local_facts_roll_back_every_child() {
    let ledger = ledger();
    let mut value: crate::node::Snapshot = serde_json::from_str(r#"{"sampled_ms":7,"probe":{"host":"value","label":"value","load1":0.25,"cores":7,"cpu_ticks":{"busy":7,"total":7},"busy":0.25,"memory":{"used_bytes":7,"total_bytes":7,"pressure_level":7},"disk_free_percent":0.25,"agents":{"total":7,"by_status":{"key":7}},"agent_panes":[{"pane":"value","agent":"value","state":"value","name":"value","transcript":"value","transcript_modified":7,"stalled_since_ms":7,"state_since_ms":7,"signal":"value","terminal_id":"value","agent_session":"value","completion_seq":7,"state_change_seq":7,"cwd":"value","session":{"model":"value","response_model":"value","thinking":"value","activity":"value"},"worktree":{"path":"value","branch":"value","ahead":7,"dirty":true,"landed":true,"head":"value","base":"value"},"repository":"value","done":true,"finishing":true,"handoff":"value","launch_progress":"value","owns_open":true,"lifecycle":"value","lifecycle_gap":"value"}],"reservations":[{"id":"value","created_ms":7}],"panes":[{"pane_id":"value","focused":true,"workspace_id":"value","name":"value","agent":"value","agent_status":"value","cwd":"value","foreground_cwd":"value","idle_shell":true,"idle_since_ms":7}]},"jobs":[{"id":"value","cmd":["value"],"cwd":"value","host":"value","owner":"value","pane":"value","state":"value","exit":7,"signal":7,"reason":"value","started_ms":7,"ended_ms":7,"timeout_ms":7,"idle_timeout_ms":7,"validity":{"version":7,"created_ms":7},"startup":{"version":1,"scope":"non_repository","cwd":"/tmp/value","preparation":null},"output_expired":true}]}"#).unwrap();
    value.sampled_ms = u64::MAX;
    assert!(put_snapshot(&ledger, &value).is_err());
    assert!(snapshot(&ledger).unwrap().is_none());
    std::fs::remove_dir_all(ledger.dir).unwrap();
}
