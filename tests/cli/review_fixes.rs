use super::watchdog::watchdog_rig;
use super::*;

#[test]
fn removal_refuses_identity_changed_during_planning() {
    let rig = watchdog_rig("identity-race");
    fs::write(rig.dir.join("change-during-plan"), "").unwrap();
    fs::write(rig.dir.join("remove-task"), "").unwrap();
    let out = rig.run(&["prune", "--yes", "worktrees"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("identity changed"));
    assert!(!rig.log("git.log").contains("worktree remove --"));
    assert!(!rig.log("mise.log").contains("run -q agent:worktree-remove"));
}

#[test]
fn low_disk_missing_and_unreadable_paths_preserve_other_candidates() {
    let rig = watchdog_rig("missing-disk");
    let missing = rig.dir.join("home/repo/missing");
    let unreadable = rig.dir.join("home/repo/unreadable");
    fs::create_dir_all(&unreadable).unwrap();
    let mut trees = rig.read("state/worktrees.json");
    for path in [&missing, &unreadable] {
        trees
            .as_array_mut()
            .unwrap()
            .push(json!({"repo":rig.dir.join("home/repo"),"path":path,"branch":"old"}));
    }
    rig.put("state/worktrees.json", &trees);
    rig.script("du", "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$RIG/du.log\"\ncase \"$*\" in *unreadable*) echo denied >&2; exit 1 ;; esac\nprintf '42 path\\n'\n");
    let (code, plan) = rig.json(&["doctor"]);
    assert_eq!(code, 0, "{plan}");
    assert!(
        plan["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["class"] == "worktrees" && e["id"] == json!(missing))
    );
    assert!(plan.to_string().contains("unreadable"));
    assert!(plan.to_string().contains("largest wrangle-owned paths"));
    assert!(!rig.log("du.log").contains(missing.to_str().unwrap()));
    assert!(rig.run(&["prune", "--yes", "processes"]).status.success());
}

#[test]
fn activity_uses_transcript_not_pi_footer_and_sanitizes_screen_and_jobs() {
    let rig = watchdog_rig("activity");
    let path = rig.dir.join("transcript.jsonl");
    fs::write(&path, format!("{}\n{}\n",json!({"type":"message","message":{"role":"assistant","content":[{"type":"toolCall","name":"bash","arguments":{"command":"cat /tmp/testa/hang.fifo"}}]}}),json!({"type":"custom","data":"tokens/footer"}))).unwrap();
    rig.put("agents.json", &json!({"result":{"agents":[{"pane_id":"child","agent":"pi","agent_status":"working","agent_session":{"kind":"path","value":path}}]}}));
    let (_, text) = rig.json(&["host", "tail", "agent", "child"]);
    assert!(text.as_str().unwrap().contains("cat /tmp/testa/hang.fifo"));
    assert!(!rig.log("herdr.log").contains("pane read"));
    rig.script("herdr", "#!/bin/sh\ncase \"$*\" in 'agent list') printf '{\"result\":{\"agents\":[]}}\\n' ;; 'pane read'*) printf '\\033[31mtool active\\033[0m\\n' ;; esac\n");
    let (_, text) = rig.json(&["host", "tail", "agent", "unknown"]);
    assert_eq!(text, "tool active");
    fs::create_dir_all(rig.dir.join("state/jobs")).unwrap();
    fs::write(
        rig.dir.join("state/jobs/test.log"),
        "\x1b[31mjob output\x1b[0m\n",
    )
    .unwrap();
    assert_eq!(rig.json(&["host", "tail", "job", "test"]).1, "job output");
}
