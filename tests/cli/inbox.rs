use super::*;

/// Herdr's agent list and this host's snapshot, both with terminal ids.
fn agents(rig: &Rig, state: &str, panes: &[(&str, &str, &str)]) {
    let listed: Vec<Value> = panes
        .iter()
        .map(|(pane, terminal, name)| {
            json!({"pane_id":pane,"agent":"pi","agent_status":"idle","name":name,"terminal_id":terminal})
        })
        .collect();
    let published: Vec<Value> = panes
        .iter()
        .map(|(pane, terminal, name)| {
            json!({"pane":pane,"agent":"pi","state":"idle","name":name,"transcript":null,
                "transcript_modified":null,"terminal_id":terminal})
        })
        .collect();
    rig.put("agents.json", &json!({"result":{"agents":listed}}));
    fs::create_dir_all(rig.dir.join(state)).unwrap();
    rig.put(
        &format!("{state}/host.json"),
        &json!({"sampled_ms":now(),"probe":{"host":"local","label":"local","load1":0.0,"cores":8,
            "memory":{"used_bytes":0,"total_bytes":1},"disk_free_percent":50.0,
            "agents":{"total":panes.len(),"by_status":{}},"agent_panes":published,
            "reservations":[],"panes":[]}}),
    );
}

fn as_pane(rig: &Rig, pane: &str, args: &[&str]) -> (i32, Value) {
    let out = rig
        .cmd(args)
        .arg("--json")
        .env("HERDR_PANE_ID", pane)
        .output()
        .unwrap();
    let value = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "{e}: {} {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    });
    (out.status.code().unwrap(), value)
}

#[test]
fn send_reaches_only_a_registered_reader_and_ack_ends_redelivery() {
    let rig = Rig::new("inbox-local");
    agents(
        &rig,
        "state",
        &[
            ("w1:p1", "term_lead", "lead"),
            ("w1:p2", "term_work", "worker"),
        ],
    );
    // No reader yet: nothing is stored, and the sender hears why.
    let (code, refused) = rig.json(&["send", "worker", "--message", "hi"]);
    assert_eq!(code, 2);
    assert!(
        refused["error"]
            .as_str()
            .unwrap()
            .contains("no wrangle inbox reader"),
        "{refused}"
    );
    let (code, claimed) = as_pane(&rig, "w1:p2", &["inbox", "claim", "--harness", "pi"]);
    assert_eq!((code, claimed), (0, json!([])));
    let (code, sent) = rig.json(&["send", "worker", "--message", "please review"]);
    assert_eq!(code, 0, "{sent}");
    assert_eq!(sent["pane"], "w1:p2");
    assert_eq!(sent["from"]["pane"], "w1:p1");
    assert_eq!(sent["from"]["name"], "lead");
    assert_eq!(sent["from"]["terminal"], "term_lead");
    let (_, claimed) = as_pane(&rig, "w1:p2", &["inbox", "claim", "--harness", "pi"]);
    assert_eq!(claimed.as_array().unwrap().len(), 1);
    assert_eq!(claimed[0]["text"], "please review");
    assert_eq!(claimed[0]["state"], "claimed");
    // The reader shows `message`; the sender and kind come from wrangle, not the harness.
    assert_eq!(
        claimed[0]["message"],
        "wrangle message from lead (w1:p1 on ".to_owned()
            + sent["from"]["host"].as_str().unwrap()
            + "):\nplease review"
    );
    assert!(
        claimed[0]["sender"]
            .as_str()
            .unwrap()
            .starts_with("lead (w1:p1 on ")
    );
    let (_, path) = as_pane(&rig, "w1:p2", &["inbox", "path"]);
    assert_eq!(path["file"], "term_work.json");
    assert!(
        std::path::Path::new(path["dir"].as_str().unwrap())
            .join("term_work.json")
            .is_file()
    );
    // The lease holds the record while the reader shows it.
    let (_, again) = as_pane(&rig, "w1:p2", &["inbox", "claim", "--harness", "pi"]);
    assert_eq!(again, json!([]));
    let id = claimed[0]["id"].as_str().unwrap();
    let (code, acked) = as_pane(&rig, "w1:p2", &["inbox", "ack", id]);
    assert_eq!((code, acked), (0, json!({"acked":1})));
    let (_, listing) = as_pane(&rig, "w1:p2", &["inbox", "list"]);
    assert_eq!(listing["received"][0]["state"], "delivered");
    // The panel shows these lines as they are; wrangle owns the state wording.
    assert_eq!(listing["lines"][0]["direction"], "received");
    assert_eq!(listing["lines"][0]["state"], "delivered");
    assert_eq!(listing["lines"][0]["summary"], "please review");
    assert_eq!(listing["lines"][0]["open"], false);
    let (_, listing) = rig.json(&["inbox", "list"]);
    assert_eq!(listing["sent"][0]["id"], id);
    // Nothing typed into any pane.
    assert!(!rig.log("herdr.log").contains("agent prompt"));
}

#[test]
fn remote_put_goes_through_ssh_and_a_remote_failure_returns_to_the_sender() {
    let rig = Rig::new("inbox-remote");
    rig.peer();
    rig.config(&format!(
        "remote_command = ['{}']\n",
        env!("CARGO_BIN_EXE_wrangle")
    ));
    // The peer runs the real binary against its own state directory.
    rig.script(
        "ssh",
        r#"#!/bin/sh
printf '%s\n' "$*" >> "$RIG/ssh.log"
shift 7
case "$1" in
  *'host snapshot'*) cat "$RIG/remote.json" ;;
  *) WRANGLE_STATE="$RIG/remote-state" sh -c "$1" ;;
esac
"#,
    );
    agents(&rig, "remote-state", &[("w9:p1", "term_far", "far")]);
    let remote = rig.read("remote-state/host.json");
    rig.put("remote.json", &remote);
    agents(&rig, "state", &[("w1:p1", "term_lead", "lead")]);

    let (code, refused) = rig.json(&["send", "far", "--message", "hi"]);
    assert_eq!(code, 2);
    assert!(
        refused["error"]
            .as_str()
            .unwrap()
            .contains("no wrangle inbox reader"),
        "{refused}"
    );
    fs::create_dir_all(rig.dir.join("remote-state/inbox")).unwrap();
    rig.put(
        "remote-state/inbox/term_far.reader.json",
        &json!({"harness":"pi","pane":"w9:p1","registered_ms":1}),
    );
    let (code, sent) = rig.json(&["send", "far", "--host", "netcup", "--message", "it's done"]);
    assert_eq!(code, 0, "{sent}");
    assert_eq!(sent["host_label"], "netcup");
    let stored = rig.read("remote-state/inbox/term_far.json");
    assert_eq!(stored[0]["text"], "it's done");
    assert_eq!(stored[0]["from"]["pane"], "w1:p1");
    assert!(rig.log("ssh.log").contains("host inbox-put --record"));

    // The sender reads its own inbox, so it can hear of the failure.
    as_pane(&rig, "w1:p1", &["inbox", "claim", "--harness", "pi"]);
    // The far terminal ends; its host fails the record on its next pass.
    let mut records = stored;
    records[0]["state"] = json!("failed");
    records[0]["reason"] = json!("its terminal is gone");
    records[0]["done_ms"] = json!(now());
    rig.put("remote-state/inbox/term_far.json", &records);
    let out = rig.run(&["serve", "--once", "--interval", "1"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(rig.log("ssh.log").contains("host inbox-state term_far"));
    let (_, claimed) = as_pane(&rig, "w1:p1", &["inbox", "claim", "--harness", "pi"]);
    assert_eq!(claimed[0]["kind"], "failed", "{claimed}");
    assert!(
        claimed[0]["text"]
            .as_str()
            .unwrap()
            .contains("to far (w9:p1 on netcup) was not delivered: its terminal is gone")
    );
    let (_, listing) = rig.json(&["inbox", "list"]);
    assert_eq!(listing["sent"][0]["state"], "failed");
}
