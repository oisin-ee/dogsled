use super::*;

#[test]
fn session_ids_resolve_unique_documented_paths_and_ambiguous_ids_stay_unstalled() {
    for agent in ["claude", "codex"] {
        let rig = Rig::new(&format!("transcript-{agent}"));
        let id = "11111111-2222-3333-4444-555555555555";
        let (root, relative) = if agent == "claude" {
            (".claude", format!("projects/project/{id}.jsonl"))
        } else {
            (
                ".codex",
                format!("sessions/2026/10/13/rollout-2026-10-13T00-00-00-{id}.jsonl"),
            )
        };
        let home = rig.dir.join("home").join(root);
        let path = home.join(&relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "record\n").unwrap();
        rig.put("agents.json",&json!({"result":{"agents":[{"pane_id":"w1:p1","agent":agent,"agent_status":"working","agent_session":{"kind":"id","value":id}}]}}));
        let run = || {
            rig.cmd(&["serve", "--once", "--interval", "1"])
                .env("CLAUDE_CONFIG_DIR", rig.dir.join("home/.claude"))
                .env("CODEX_HOME", rig.dir.join("home/.codex"))
                .output()
                .unwrap()
        };
        assert!(run().status.success());
        assert_eq!(
            rig.read("state/host.json")["probe"]["agent_panes"][0]["transcript"],
            json!(path)
        );
        let duplicate = if agent == "claude" {
            home.join(format!("projects/copy/{id}.jsonl"))
        } else {
            home.join(format!(
                "sessions/2026/10/14/rollout-2026-10-14T00-00-00-{id}.jsonl"
            ))
        };
        fs::create_dir_all(duplicate.parent().unwrap()).unwrap();
        fs::write(duplicate, "copy\n").unwrap();
        assert!(run().status.success());
        let pane = &rig.read("state/host.json")["probe"]["agent_panes"][0];
        assert!(pane["transcript"].is_null());
        assert!(pane["stalled_since_ms"].is_null());
    }
}
