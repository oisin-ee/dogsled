use super::*;

#[test]
fn machine_changes_run_herdr_and_prompts_open_in_their_own_tab() {
    let rig = Rig::new("machines");
    rig.peer();
    let (code, renamed) = rig.json(&["machine", "rename", "netcup", "netcup-2"]);
    assert_eq!(code, 0, "{renamed}");
    assert!(
        rig.log("herdr.log")
            .contains("machine rename --label netcup-2 8103")
    );
    let (code, _) = rig.json(&["machine", "disable", "8103"]);
    assert_eq!(code, 0);
    assert!(rig.log("herdr.log").contains("machine disable 8103"));
    let (code, refused) = rig.json(&["machine", "rename", "netcup", "a b"]);
    assert_eq!(code, 2, "{refused}");
    assert!(!rig.log("herdr.log").contains("a b"));
    // Reconnect may ask for a password, so it runs in a new focused tab, never in an agent pane.
    let (code, opened) = rig
        .cmd(&["--json", "machine", "reconnect", "netcup"])
        .env("HERDR_WORKSPACE_ID", "w1")
        .output()
        .map(|o| {
            (
                o.status.code().unwrap(),
                serde_json::from_slice::<Value>(&o.stdout).unwrap_or(Value::Null),
            )
        })
        .unwrap();
    assert_eq!(code, 0);
    assert_eq!(opened["pane"], "w2:p1");
    let log = rig.log("herdr.log");
    assert!(log.contains("tab create --workspace w1"), "{log}");
    assert!(log.contains("--focus"), "{log}");
    // The new pane runs `herdr machine reconnect <id>`, which the fake herdr logs.
    assert!(
        log.contains("pane run w2:p1 herdr machine reconnect 8103"),
        "{log}"
    );
    assert!(rig.log("herdr.log").contains("\nmachine reconnect 8103\n"));
}
