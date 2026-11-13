use super::super::{Publisher, collect::Collector, peers_runtime::Observers};
use crate::{
    config::Policy,
    hosts::{Fleet, Host},
    ledger::Ledger,
    node::Node,
    shell::{Fake, Shell, System},
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

#[path = "../../../tests/fixtures/observation/load.rs"]
mod fixtures;

const NOW: u64 = 10_000;

#[derive(Debug)]
struct Source(Fake);

fn isolated(program: &str, args: &[String]) -> Vec<String> {
    let mut words = Vec::new();
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("GIT_") {
            words.extend(["-u".to_owned(), key.to_string_lossy().into_owned()]);
        }
    }
    words.extend([
        "GIT_CONFIG_NOSYSTEM=1".to_owned(),
        "GIT_CONFIG_GLOBAL=/dev/null".to_owned(),
        program.to_owned(),
    ]);
    words.extend_from_slice(args);
    words
}

impl Shell for Source {
    fn run(&self, program: &str, args: &[String]) -> crate::error::Result<crate::shell::Output> {
        if program == "git" || program == "env" {
            System::default().run("env", &isolated(program, args))
        } else {
            self.0.run(program, args)
        }
    }
    fn run_bytes_bounded(
        &self,
        program: &str,
        args: &[String],
        timeout: std::time::Duration,
        limit: usize,
    ) -> crate::error::Result<crate::shell::BytesOutput> {
        System::default().run_bytes_bounded("env", &isolated(program, args), timeout, limit)
    }
    fn run_input_bytes_bounded(
        &self,
        program: &str,
        args: &[String],
        input: &[u8],
        timeout: std::time::Duration,
        limit: usize,
    ) -> crate::error::Result<crate::shell::BytesOutput> {
        System::default().run_input_bytes_bounded(
            "env",
            &isolated(program, args),
            input,
            timeout,
            limit,
        )
    }
    fn run_input_bounded(
        &self,
        program: &str,
        args: &[String],
        input: &[u8],
        timeout: std::time::Duration,
    ) -> crate::error::Result<crate::shell::Output> {
        if program == "git" || program == "env" {
            System::default().run_input_bounded("env", &isolated(program, args), input, timeout)
        } else {
            self.0.run_input_bounded(program, args, input, timeout)
        }
    }
    fn read_file(&self, path: &str) -> crate::error::Result<String> {
        self.0.read_file(path)
    }
}

struct Fixture {
    root: PathBuf,
    ledger: Ledger,
    source: Source,
    policy: Policy,
}

fn git(path: &Path, args: &[&str]) {
    let mut command = std::process::Command::new("git");
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("GIT_") {
            command.env_remove(key);
        }
    }
    let output = command
        .current_dir(path)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_DATE", "2000-01-01T00:00:00Z")
        .env("GIT_COMMITTER_DATE", "2000-01-01T00:00:00Z")
        .args([
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(crate::ledger::new_id());
        std::fs::create_dir_all(&root).unwrap();
        let root = std::fs::canonicalize(root).unwrap();
        let repo = root.join("repo");
        let tree = root.join("worker");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        git(&repo, &["commit", "-q", "--allow-empty", "-m", "base"]);
        git(
            &repo,
            &[
                "remote",
                "add",
                "origin",
                "https://github.com/fixture/context.git",
            ],
        );
        git(&repo, &["update-ref", "refs/remotes/origin/main", "main"]);
        git(
            &repo,
            &[
                "symbolic-ref",
                "refs/remotes/origin/HEAD",
                "refs/remotes/origin/main",
            ],
        );
        git(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "feat/x",
                tree.to_str().unwrap(),
            ],
        );
        for message in ["first", "second"] {
            std::fs::write(tree.join(message), format!("{message}\n")).unwrap();
            git(&tree, &["add", message]);
            git(&tree, &["commit", "-q", "-m", message]);
        }
        let ledger = Ledger::new(root.join("state"));
        let mut items = Vec::new();
        for (id, owner, name) in [("mine", "w1:p1", "worker"), ("theirs", "w9:p9", "other")] {
            items.push(crate::items::Item {
                id: id.into(),
                kind: "agent".into(),
                host: "local".into(),
                pane: "w2:p1".into(),
                owner_pane: owner.into(),
                name: Some(name.into()),
                created_ms: 1,
                ..crate::items::Item::default()
            });
        }
        crate::store::put_items(&ledger, &items).unwrap();
        let mark = serde_json::from_value(json!({"pane":"w2:p1","item":null,"session":"worker-session","marked_ms":1,"after":0,"sealed":1,"native_done":true,"reply":{"seq":1,"text":"Finished","captured_ms":1,"done":true}})).unwrap();
        crate::host_store::put_marks(&ledger, &BTreeMap::from([("worker".into(), mark)])).unwrap();
        let source = Source(Fake::new());
        source.0.on("herdr pane list", r#"{"result":{"panes":[]}}"#)
            .on("herdr agent list", &json!({"result":{"agents":[{"pane_id":"w2:p1","agent":"pi","agent_status":"idle","name":"worker","terminal_id":"term_worker","agent_session":{"kind":"id","value":"worker-session"},"completion_seq":1,"state_change_seq":1,"cwd":tree}]}}).to_string())
            .on("read /proc/stat", "cpu 100 20 30 400 50 6 7 8 9 10")
            .on("read /proc/loadavg", "1 1 1 1/10 42")
            .on("read /proc/meminfo", "MemTotal: 8388608 kB\nMemAvailable: 7340032 kB")
            .on("df -Pk /home", "FS 1000 500 500 50% /")
            .on("sysctl -n vm.loadavg", "{ 1.0 1.0 1.0 }")
            .on("iostat -c 2 -w 1", "us sy id 1m 5m 15m\n1 2 97 1 1 1\n1 2 97 1 1 1\n")
            .on("pi --mode rpc --no-session", r#"{"id":"models","type":"response","success":true,"data":{"models":[{"type":"chat","provider":"fixture","id":"chat","name":"Fixture Chat","reasoning":false}]}}"#)
            .on("sysctl -n hw.memsize", "8589934592\n")
            .on("sysctl -n kern.memorystatus_vm_pressure_level", "1\n")
            .on("vm_stat", "Mach Virtual Memory Statistics: (page size of 16384 bytes)\nPages free: 458752.\nPages inactive: 0.\nPages speculative: 0.\n");
        Self {
            root,
            ledger,
            source,
            policy: Policy::default(),
        }
    }
    fn publication(&self) -> Value {
        let node = Node::new(&self.source, &self.ledger, &self.policy, "/home");
        let mut collector = Collector::default();
        let mut input = collector.collect(&node, NOW).input;
        assert_eq!(input.gaps, []);
        let mut reports = collector.reports();
        let report = &mut reports[0];
        let probe = report.probe.as_mut().unwrap();
        assert!(probe.agent_panes[0].done);
        assert_eq!(probe.agent_panes[0].handoff.as_deref(), Some("recorded"));
        if probe.agent_panes[0].worktree.is_none() {
            let tree = self.root.join("worker");
            let result = crate::loss::Evidence::default_only(tree.to_str().unwrap()).assess(
                &self.source,
                "refs/heads/feat/x",
                None,
            );
            panic!("worktree observation absent; loss assessment: {result:?}");
        }
        assert_eq!(probe.agent_panes[0].worktree.as_ref().unwrap().ahead, 2);
        // Host CPU counters, capacity and pressure representation vary by platform.
        probe.cpu_ticks = Some(serde_json::from_value(json!({"busy":171,"total":621})).unwrap());
        probe.memory.pressure_level = None;
        probe.cores = 8;
        probe.busy = Some(0.125);
        report.headroom = Some(crate::plan::evaluate(probe, &node.thresholds));
        input.local.get_mut("host").unwrap()["probe"] = serde_json::to_value(probe).unwrap();
        input.local.insert(
            "headroom".into(),
            serde_json::to_value(&report.headroom).unwrap(),
        );
        input.local.insert(
            "status".into(),
            serde_json::to_value(
                super::super::collect::status(&self.ledger, reports.clone(), NOW).unwrap(),
            )
            .unwrap(),
        );
        let fleet = Fleet::cold(
            &self.source,
            &self.ledger,
            &self.policy,
            "/home".into(),
            vec![Host::local()],
        );
        Observers::default()
            .compose(&fleet, &mut input, NOW, reports)
            .unwrap();
        let frame = Publisher::new().unwrap().replace(input).unwrap();
        let mut value = serde_json::to_value(&*frame).unwrap();
        value["epoch"] = json!("context-pre-handoff");
        normalize(&mut value, self.root.to_str().unwrap());
        value
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.root).unwrap();
    }
}

fn normalize(value: &mut Value, root: &str) {
    match value {
        Value::String(text) => *text = text.replace(root, "/fixture/context"),
        Value::Array(values) => {
            for value in values {
                normalize(value, root);
            }
        }
        Value::Object(values) => {
            for (key, value) in values.iter_mut() {
                if key == "epoch" {
                    *value = json!("context-pre-handoff");
                } else {
                    normalize(value, root);
                }
            }
        }
        _ => (),
    }
}

#[test]
fn context_pre_handoff_snapshot_matches_real_capture_and_compose() {
    let fixture = Fixture::new();
    let publication = fixture.publication();
    let boards = &publication["views"]["boards"]["data"];
    for (owner, child, other) in [("w1:p1", "mine", "theirs"), ("w9:p9", "theirs", "mine")] {
        assert_eq!(
            boards[owner]["rows"]
                .as_array()
                .unwrap_or_else(|| panic!("{owner}: {publication}"))
                .len(),
            1
        );
        assert_eq!(boards[owner]["rows"][0]["id"], child);
        assert_eq!(boards[owner]["rows"][0]["group"], "to_land");
        assert!(!boards[owner].to_string().contains(other));
        assert_eq!(
            publication["views"]["findings"]["data"][owner],
            json!({"failures":[],"holds":[],"landed_dirty":[]})
        );
    }
    let display: super::super::display::Display =
        serde_json::from_value(publication["views"]["status_display"]["data"].clone()).unwrap();
    assert!(display.brief(NOW).contains("(2 finished)"));
    let mut bytes = serde_json::to_vec_pretty(&publication).unwrap();
    bytes.push(b'\n');
    // Explicit generation only. A normal test never rewrites its expected publication.
    if std::env::var_os("DOGSLED_UPDATE_CONTEXT_PUBLICATION").is_some() {
        std::fs::write(fixtures::context_path(), &bytes).unwrap();
    }
    assert_eq!(bytes, std::fs::read(fixtures::context_path()).unwrap());
    assert_eq!(publication, fixtures::context_pre_handoff());
}
