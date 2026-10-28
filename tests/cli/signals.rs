use super::*;

fn agents(rig: &Rig, state: &str, seq: u64, session: &str, owner_session: &str) {
    rig.put("agents.json", &json!({"result":{"agents":[
        {"pane_id":"w1:p1","agent":"pi","agent_status":"idle","terminal_id":"term_owner",
         "agent_session":{"kind":"path","value":owner_session}},
        {"pane_id":"w2:p1","agent":"pi","agent_status":state,"name":"worker","terminal_id":"term_child",
         "agent_session":{"kind":"path","value":session},"state_change_seq":seq,"state_since_ms":seq * 100},
        {"pane_id":"w3:p1","agent":"pi","agent_status":"working","terminal_id":"term_grandchild",
         "agent_session":{"kind":"path","value":"grandchild-session"}}
    ]}}));
    // Every listed agent also has a live pane, as it does after a real spawn.
    // Reconciliation uses pane list, not agent list, to prove that a child is gone.
    rig.put(
        "panes.json",
        &json!({"result":{"panes":[
            {"pane_id":"w1:p1","terminal_id":"term_owner","agent":"pi"},
            {"pane_id":"w2:p1","terminal_id":"term_child","agent":"pi"},
            {"pane_id":"w3:p1","terminal_id":"term_grandchild","agent":"pi"}
        ]}}),
    );
}

fn setup(name: &str) -> Rig {
    let rig = Rig::new(name);
    agents(&rig, "working", 10, "child-session", "owner-session");
    rig.put(
        "state/items.json",
        &json!([{
            "id":"child","kind":"agent","host":"local","pane":"w2:p1","owner_pane":"w1:p1",
            "owner_session":"owner-session","name":"worker","created_ms":1,"closed_ms":null
        }]),
    );
    rig.snapshot("state/host.json", now(), 0.0, 8);
    let out = rig.run(&["inbox", "claim", "--harness", "pi"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    rig
}

fn signal(rig: &Rig, kind: &str, text: &str) -> (i32, Value) {
    let out = rig
        .cmd(&[kind, text, "--json"])
        .env("HERDR_PANE_ID", "w2:p1")
        .output()
        .unwrap();
    let value = serde_json::from_slice(&out.stdout).unwrap();
    (out.status.code().unwrap(), value)
}

fn serve(rig: &Rig) {
    let out = rig.run(&["serve", "--once", "--interval", "1"]);
    let journal = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{journal}");
    assert!(!journal.contains("dogsled service lane "), "{journal}");
    // Keep the published host live while subsequent CLI readers inspect its snapshot.
    rig.service();
}

fn notes(rig: &Rig, kind: &str) -> Vec<Value> {
    if !rig.dir.join("state/inbox/term_owner.json").exists() {
        return Vec::new();
    }
    let records = rig.read("state/inbox/term_owner.json");
    records
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["kind"] == kind)
        .cloned()
        .collect()
}

fn row(rig: &Rig) -> Value {
    let (code, board) = rig.json(&["board", "--owner", "w1:p1"]);
    assert_eq!(code, 0, "{board}");
    board["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == "child")
        .unwrap()
        .clone()
}

#[test]
fn ask_reaches_owner_item_and_inbox_once_and_next_turn_clears_it() {
    let rig = setup("signal-ask");
    let (code, event) = signal(&rig, "ask", "Which branch should I use?");
    assert_eq!(code, 0, "{event}");
    let (code, repeat) = signal(&rig, "ask", "Which branch should I use?");
    assert_eq!(code, 0, "{repeat}");
    assert_eq!(repeat["event_id"], event["event_id"]);
    agents(&rig, "idle", 11, "child-session", "owner-session");
    serve(&rig);
    serve(&rig);
    let item = &rig.read("state/items.json")[0];
    assert_eq!(item["signal"]["event_id"], event["event_id"]);
    assert!(item["signal"]["reported_ms"].is_u64(), "{item}");
    assert!(
        item["reply"].is_null() && item["closed_ms"].is_null(),
        "{item}"
    );
    let records = notes(&rig, "ask");
    assert_eq!(records.len(), 1, "{records:?}");
    assert_eq!(records[0]["text"], "Which branch should I use?");
    assert_eq!(row(&rig)["group"], "attention");
    assert_eq!(row(&rig)["activity"], "Which branch should I use?");
    agents(&rig, "working", 12, "child-session", "owner-session");
    serve(&rig);
    assert!(rig.read("state/items.json")[0]["signal"].is_null());
    assert_ne!(row(&rig)["activity"], "Which branch should I use?");
    let mark = rig.read("state/done.json");
    let mark = mark.as_object().unwrap().values().next().unwrap();
    assert!(mark["signal"]["cleared_ms"].is_u64(), "{mark}");
    assert!(
        mark["sealed"].is_null() && mark["reply"].is_null(),
        "{mark}"
    );
}

#[test]
fn blocked_reason_shows_under_needs_you_and_next_turn_clears_it() {
    let rig = setup("signal-blocked");
    let (code, event) = signal(&rig, "blocked", "Credentials are unavailable");
    assert_eq!(code, 0, "{event}");
    agents(&rig, "idle", 11, "child-session", "owner-session");
    serve(&rig);
    assert_eq!(row(&rig)["group"], "attention");
    assert_eq!(row(&rig)["state"], "blocked");
    assert_eq!(row(&rig)["activity"], "Credentials are unavailable");
    assert_eq!(notes(&rig, "blocked").len(), 1);
    agents(&rig, "working", 12, "child-session", "owner-session");
    serve(&rig);
    assert!(rig.read("state/items.json")[0]["signal"].is_null());
    assert_eq!(row(&rig)["state"], "working");
}

#[test]
fn replacement_owner_session_never_receives_the_old_question() {
    let rig = setup("signal-replacement-owner");
    // No custody record exists yet: delivery must establish it before the live check.
    assert_eq!(signal(&rig, "ask", "Old owner's question").0, 0);
    agents(&rig, "idle", 11, "child-session", "replacement-session");
    serve(&rig);
    assert_eq!(notes(&rig, "ask"), Vec::<Value>::new());
    assert!(rig.read("state/items.json")[0]["signal"]["reported_ms"].is_null());
}

#[test]
fn replacement_child_session_never_sees_or_clears_an_old_signal() {
    let rig = setup("signal-replacement-child");
    assert_eq!(signal(&rig, "ask", "Old child question").0, 0);
    agents(&rig, "working", 30, "replacement-child", "owner-session");
    serve(&rig);
    assert!(rig.read("state/items.json")[0]["signal"].is_null());
    assert_eq!(notes(&rig, "ask"), Vec::<Value>::new());
    let marks = rig.read("state/done.json");
    let mark = marks.as_object().unwrap().values().next().unwrap();
    assert!(mark["signal"]["cleared_ms"].is_null(), "{mark}");
}

#[test]
fn ask_works_with_dirty_work_and_open_children_and_does_not_close_input() {
    let (rig, tree) = super::close::close_rig("signal-dirty-children");
    agents(&rig, "working", 10, "child-session", "owner-session");
    let mut listed = rig.read("agents.json");
    listed["result"]["agents"][1]["cwd"] = json!(tree);
    rig.put("agents.json", &listed);
    fs::write(tree.join("scratch"), "uncommitted work").unwrap();
    rig.put("state/items.json", &json!([
        {"id":"child","kind":"agent","host":"local","pane":"w2:p1","owner_pane":"w1:p1","created_ms":1,"closed_ms":null},
        {"id":"grandchild","kind":"agent","host":"local","pane":"w3:p1","owner_pane":"w2:p1","owner_session":"child-session","created_ms":1,"closed_ms":null}
    ]));
    let (code, event) = signal(&rig, "ask", "Can I expand scope?");
    assert_eq!(code, 0, "{event}");
    let mark = rig.read("state/done.json");
    let mark = mark.as_object().unwrap().values().next().unwrap();
    assert!(
        mark["sealed"].is_null() && mark["reply"].is_null(),
        "{mark}"
    );
    // Signal-only records leave the normal input route open.
    rig.snapshot("state/host.json", now(), 0.0, 8);
    let mut host = rig.read("state/host.json");
    host["probe"]["agent_panes"] = json!([{"pane":"w2:p1","agent":"pi","state":"working","terminal_id":"term_child","name":"worker"}]);
    rig.put("state/host.json", &host);
    let out = rig
        .cmd(&["inbox", "claim", "--harness", "pi"])
        .env("HERDR_PANE_ID", "w2:p1")
        .output()
        .unwrap();
    assert!(out.status.success());
    let (code, sent) = rig.json(&["send", "worker", "--message", "Yes, expand scope"]);
    assert_eq!(code, 0, "{sent}");
    assert_eq!(
        rig.read("state/inbox/term_child.json")[0]["text"],
        "Yes, expand scope"
    );
}

#[test]
fn a_signal_reaches_the_current_custodian_once_even_after_custody_moves() {
    let rig = setup("signal-custodian");
    let mut listed = rig.read("agents.json");
    listed["result"]["agents"][2]["agent_status"] = json!("idle");
    listed["result"]["agents"][2]["agent_session"] =
        json!({"kind":"path","value":"custodian-session"});
    rig.put("agents.json", &listed);
    rig.put("state/custody.json", &json!([{
        "item":"child","host":"local","owner_pane":"w1:p1","owner_session":"owner-session",
        "custodian_pane":"w3:p1","custodian_session":"custodian-session","revision":1,"claims":[]
    }]));
    let out = rig
        .cmd(&["inbox", "claim", "--harness", "pi"])
        .env("HERDR_PANE_ID", "w3:p1")
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_eq!(signal(&rig, "ask", "Custodian question").0, 0);
    serve(&rig);
    serve(&rig);
    assert_eq!(notes(&rig, "ask"), Vec::<Value>::new());
    let records = rig.read("state/inbox/term_grandchild.json");
    let questions: Vec<_> = records
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["kind"] == "ask")
        .collect();
    assert_eq!(questions.len(), 1, "{records}");
    assert_eq!(questions[0]["text"], "Custodian question");
    let mut custody = rig.read("state/custody.json");
    custody[0]["custodian_pane"] = json!("w1:p1");
    custody[0]["custodian_session"] = json!("owner-session");
    rig.put("state/custody.json", &custody);
    serve(&rig);
    assert_eq!(notes(&rig, "ask"), Vec::<Value>::new());
}

#[test]
fn remote_snapshot_imports_and_clears_the_signal() {
    let rig = setup("signal-remote");
    let (code, event) = signal(&rig, "ask", "Remote question");
    assert_eq!(code, 0);
    let marks = rig.read("state/done.json");
    let mut mark = marks.as_object().unwrap().values().next().unwrap().clone();
    mark["pane"] = json!("w9:p1");
    rig.put("state/done.json", &json!({}));
    let mut listed = rig.read("agents.json");
    listed["result"]["agents"]
        .as_array_mut()
        .unwrap()
        .retain(|a| a["pane_id"] != "w2:p1");
    rig.put("agents.json", &listed);
    let mut items = rig.read("state/items.json");
    items[0]["host"] = json!("8103");
    items[0]["pane"] = json!("w9:p1");
    rig.put("state/items.json", &items);
    rig.peer();
    let mut remote = rig.read("remote.json");
    remote["probe"]["agent_panes"] = json!([{
        "pane":"w9:p1","agent":"pi","state":"idle","name":"worker",
        "agent_session":"child-session","signal":"ask","terminal_id":"term_remote"
    }]);
    rig.put(
        "remote.json",
        &json!({"snapshot":remote,"running":true,"handoffs":[mark]}),
    );
    serve(&rig);
    assert_eq!(
        rig.read("state/items.json")[0]["signal"]["event_id"],
        event["event_id"]
    );
    assert_eq!(notes(&rig, "ask").len(), 1);
    let mut report = rig.read("remote.json");
    report["handoffs"][0]["signal"]["cleared_ms"] = json!(now());
    report["snapshot"]["probe"]["agent_panes"][0]["signal"] = Value::Null;
    report["snapshot"]["probe"]["agent_panes"][0]["state"] = json!("working");
    rig.put("remote.json", &report);
    let out = rig.run(&["retry", "netcup"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serve(&rig);
    assert!(rig.read("state/items.json")[0]["signal"].is_null());
    assert_eq!(notes(&rig, "ask").len(), 1);
}

fn remote_signal(name: &str) -> (Rig, Value) {
    let rig = setup(name);
    assert_eq!(signal(&rig, "ask", "Remote outage question").0, 0);
    let marks = rig.read("state/done.json");
    let mut mark = marks.as_object().unwrap().values().next().unwrap().clone();
    mark["pane"] = json!("w9:p1");
    rig.put("state/done.json", &json!({}));
    let mut listed = rig.read("agents.json");
    listed["result"]["agents"]
        .as_array_mut()
        .unwrap()
        .retain(|a| a["pane_id"] != "w2:p1");
    rig.put("agents.json", &listed);
    let mut items = rig.read("state/items.json");
    items[0]["host"] = json!("8103");
    items[0]["pane"] = json!("w9:p1");
    rig.put("state/items.json", &items);
    rig.peer();
    let mut remote = rig.read("remote.json");
    remote["probe"]["agent_panes"] = json!([{
        "pane":"w9:p1","agent":"pi","state":"idle","name":"worker",
        "agent_session":"child-session","signal":"ask","terminal_id":"term_remote"
    }]);
    remote["probe"]["panes"] = json!([{"pane_id":"w9:p1","terminal_id":"term_remote"}]);
    remote["running"] = json!(true);
    remote["handoffs"] = json!([mark]);
    rig.put("remote.json", &remote);
    serve(&rig);
    let saved = rig.read("state/items.json")[0]["signal"].clone();
    assert!(saved["reported_ms"].is_u64(), "{saved}");
    assert_eq!(notes(&rig, "ask").len(), 1);
    (rig, saved)
}

fn refresh_remote(rig: &Rig) {
    let out = rig.run(&["retry", "netcup"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serve(rig);
}

fn remote_outage(rig: &Rig, saved: &Value) {
    fs::write(rig.dir.join("unreachable"), "offline").unwrap();
    refresh_remote(rig);
    let cache = rig.read("state/peer-38313033.json");
    assert_eq!(cache["reachable"], false, "{cache}");
    assert_eq!(cache["handoffs"], json!([]), "{cache}");
    assert_eq!(
        cache["snapshot"]["probe"]["agent_panes"][0]["agent_session"],
        "child-session"
    );
    assert_eq!(rig.read("state/items.json")[0]["signal"], *saved);
}

#[test]
fn failed_remote_read_keeps_cached_signal_and_delivery_stamp_after_recovery() {
    let (rig, saved) = remote_signal("signal-remote-outage");
    remote_outage(&rig, &saved);
    // A cached failure is no more evidence than the original failed read.
    serve(&rig);
    assert_eq!(rig.read("state/items.json")[0]["signal"], saved);
    fs::remove_file(rig.dir.join("unreachable")).unwrap();
    refresh_remote(&rig);
    assert_eq!(rig.read("state/items.json")[0]["signal"], saved);
    assert_eq!(notes(&rig, "ask").len(), 1);
}

#[test]
fn successful_remote_clear_after_outage_clears_the_retained_signal() {
    let (rig, saved) = remote_signal("signal-remote-outage-clear");
    remote_outage(&rig, &saved);
    fs::remove_file(rig.dir.join("unreachable")).unwrap();
    let mut report = rig.read("remote.json");
    report["handoffs"][0]["signal"]["cleared_ms"] = json!(now());
    rig.put("remote.json", &report);
    refresh_remote(&rig);
    assert!(rig.read("state/items.json")[0]["signal"].is_null());
    assert_eq!(notes(&rig, "ask").len(), 1);
}

#[test]
fn replacement_remote_session_clears_a_signal_without_a_matching_mark() {
    let (rig, saved) = remote_signal("signal-remote-replacement");
    let mut report = rig.read("remote.json");
    report["handoffs"] = json!([]);
    rig.put("remote.json", &report);
    refresh_remote(&rig);
    // A successful but mark-less read of the same session also proves no clearance.
    assert_eq!(rig.read("state/items.json")[0]["signal"], saved);
    report["probe"]["agent_panes"][0]["agent_session"] = json!("replacement");
    rig.put("remote.json", &report);
    refresh_remote(&rig);
    assert!(rig.read("state/items.json")[0]["signal"].is_null());
    assert_eq!(notes(&rig, "ask").len(), 1);
}

#[test]
fn outage_during_custody_move_does_not_deliver_the_same_event_twice() {
    let (rig, saved) = remote_signal("signal-remote-outage-custody");
    remote_outage(&rig, &saved);
    let mut listed = rig.read("agents.json");
    listed["result"]["agents"][1]["agent_status"] = json!("idle");
    listed["result"]["agents"][1]["agent_session"] =
        json!({"kind":"path","value":"custodian-session"});
    rig.put("agents.json", &listed);
    let out = rig
        .cmd(&["inbox", "claim", "--harness", "pi"])
        .env("HERDR_PANE_ID", "w3:p1")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let mut custody = rig.read("state/custody.json");
    custody[0]["custodian_pane"] = json!("w3:p1");
    custody[0]["custodian_session"] = json!("custodian-session");
    custody[0]["revision"] = json!(1);
    rig.put("state/custody.json", &custody);
    serve(&rig);
    fs::remove_file(rig.dir.join("unreachable")).unwrap();
    refresh_remote(&rig);
    assert_eq!(rig.read("state/items.json")[0]["signal"], saved);
    assert_eq!(notes(&rig, "ask").len(), 1);
    if rig.dir.join("state/inbox/term_grandchild.json").exists() {
        let records = rig.read("state/inbox/term_grandchild.json");
        assert_eq!(
            records
                .as_array()
                .unwrap()
                .iter()
                .filter(|r| r["kind"] == "ask")
                .count(),
            0,
            "{records}"
        );
    }
}

#[test]
fn open_children_without_a_signal_show_waiting() {
    let rig = setup("signal-waiting");
    let mut items = rig.read("state/items.json");
    items.as_array_mut().unwrap().push(json!({
        "id":"grandchild","kind":"agent","host":"local","pane":"w3:p1","owner_pane":"w2:p1",
        "owner_session":"child-session","created_ms":1,"closed_ms":null
    }));
    rig.put("state/items.json", &items);
    agents(&rig, "idle", 11, "child-session", "owner-session");
    for pass in 1..=2 {
        serve(&rig);
        let items = rig.read("state/items.json");
        let grandchild = items
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["id"] == "grandchild")
            .unwrap();
        assert!(grandchild["closed_ms"].is_null(), "serve {pass}: {items}");
        let host = rig.read("state/host.json");
        let child = host["probe"]["agent_panes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["pane"] == "w2:p1")
            .unwrap();
        assert_eq!(child["lifecycle"], "waiting", "serve {pass}: {child}");
        assert_eq!(row(&rig)["group"], "running", "serve {pass}");
        assert!(items[0]["signal"].is_null(), "serve {pass}: {items}");
    }
}
