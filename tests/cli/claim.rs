use super::*;

const ID: &str = "r-1793174082524-94752-0";

fn rig(name: &str, owner: &str) -> Rig {
    let rig = Rig::new(name);
    rig.put(
        "state/items.json",
        &json!([{
            "id":ID,"kind":"agent","host":"local","pane":"w2:p1",
            "owner_pane":owner,"owner_session":"child-session","created_ms":1,"closed_ms":null
        }]),
    );
    rig.put(
        "agents.json",
        &json!({"result":{"agents":[
            {"pane_id":"w1:p1","agent_session":{"kind":"id","value":"lead-session"}},
            {"pane_id":"w2:p1","agent_session":{"kind":"id","value":"child-session"}}
        ]}}),
    );
    rig
}

fn returned(rig: &Rig) -> Value {
    let export = super::unit_requests::exported(0);
    let execution = json!({"executor":"dogsled","ownerId":"w9:p9","requestId":ID});
    let binding = json!({"contract":export["contract"]["ref"],"execution":execution,"exportDigest":export["digest"]});
    fs::create_dir_all(rig.dir.join("state/admissions")).unwrap();
    rig.put(&format!("state/admissions/{ID}.json"), &json!({
        "request":{},"launch":null,"error":null,"unit":{
            "origin":export["contract"]["ref"],"execution":execution,"placement":{},
            "tool_call_id":"call-1","abandoned":null,
            "committed":{"execution":execution,"binding":binding,"exported":export},
            "returned":{"version":1,"eventId":format!("{ID}-returned"),"binding":binding,
                "result":{"kind":"returned","evidence":{"execution":execution,"resultId":"res-1","summary":"done"}}}
        }
    }));
    execution
}

#[test]
fn claiming_an_orphan_succeeds() {
    let rig = rig("claim-orphan", "w9:p9");
    let (code, receipt) = rig.json(&["claim", ID]);
    assert_eq!(code, 0, "{receipt}");
    assert_eq!(receipt["outcome"], "claimed");
    assert_eq!(receipt["revision"], 1);
    assert_eq!(receipt["custodianPane"], "w1:p1");
    assert_eq!(receipt["custodianSession"], "lead-session");
    assert_eq!(rig.read("state/items.json")[0]["owner_pane"], "w9:p9");
}

#[test]
fn claiming_a_non_orphan_is_refused() {
    let rig = rig("claim-live", "w2:p1");
    let (code, receipt) = rig.json(&["claim", ID]);
    assert_eq!(code, 2);
    assert!(
        receipt["error"].as_str().unwrap().contains("not_orphan"),
        "{receipt}"
    );
    assert!(!rig.dir.join("state/custody.json").exists());
}

#[test]
fn stale_revision_is_refused() {
    let rig = rig("claim-stale", "w9:p9");
    let (code, receipt) = rig.json(&["claim", ID, "--revision", "9"]);
    assert_eq!(code, 2);
    assert!(
        receipt["error"]
            .as_str()
            .unwrap()
            .contains("revision_conflict"),
        "{receipt}"
    );
    assert!(!rig.dir.join("state/custody.json").exists());
}

#[test]
fn repeat_claim_by_custodian_is_idempotent() {
    let rig = rig("claim-repeat", "w9:p9");
    let (code, first) = rig.json(&["claim", ID]);
    assert_eq!(code, 0, "{first}");
    let before = fs::read(rig.dir.join("state/custody.json")).unwrap();
    let (code, again) = rig.json(&["claim", ID, "--revision", "0"]);
    assert_eq!(code, 0, "{again}");
    assert_eq!(again["outcome"], "held");
    assert_eq!(again["claimId"], first["claimId"]);
    assert_eq!(
        fs::read(rig.dir.join("state/custody.json")).unwrap(),
        before
    );
}

#[test]
fn accept_after_claim_writes_receipt_and_leaves_item_open() {
    let rig = rig("accept-claimed", "w9:p9");
    let execution = returned(&rig);
    let items = fs::read(rig.dir.join("state/items.json")).unwrap();
    let (code, claim) = rig.json(&["claim", ID]);
    assert_eq!(code, 0, "{claim}");
    let args = [
        "accept",
        ID,
        "--result",
        "res-1",
        "--evidence",
        "checked exact result",
    ];
    let (code, receipt) = rig.json(&args);
    assert_eq!(code, 0, "{receipt}");
    assert_eq!(receipt["kind"], "accept");
    assert_eq!(receipt["resultId"], "res-1");
    assert_eq!(receipt["claimId"], claim["claimId"]);
    assert_eq!(receipt["execution"], execution);
    assert_eq!(receipt["applied"], true);
    let saved = rig.read(&format!("state/admissions/{ID}.json"));
    assert_eq!(saved["unit"]["reviews"][0]["reviewId"], receipt["reviewId"]);
    assert_eq!(fs::read(rig.dir.join("state/items.json")).unwrap(), items);
    let (code, replay) = rig.json(&args);
    assert_eq!(code, 0, "{replay}");
    assert_eq!(replay["applied"], false);
    assert_eq!(replay["reviewId"], receipt["reviewId"]);
}

#[test]
fn accept_without_custody_is_refused() {
    let rig = rig("accept-without-custody", "w9:p9");
    returned(&rig);
    let (code, receipt) = rig.json(&["accept", ID, "--result", "res-1", "--evidence", "reviewed"]);
    assert_eq!(code, 2);
    assert!(
        receipt["error"].as_str().unwrap().contains("custody"),
        "{receipt}"
    );
}
