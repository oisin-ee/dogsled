use super::super::collect::Collector;
use crate::config::Policy;
use crate::herdr_socket::fake::Server;
use crate::ledger::Ledger;
use crate::node::Node;
use crate::shell::Fake;
use serde_json::json;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

fn capacity() -> Fake {
    let shell = Fake::new();
    shell
        .on("read /proc/loadavg", "0.5 0.2 0.1 1/10 42")
        .on("read /proc/stat", "cpu 100 20 30 400 50 6 7 8")
        .on(
            "read /proc/meminfo",
            "MemTotal: 100 kB\nMemAvailable: 75 kB",
        )
        .on("df -Pk /home", "FS 1000 500 400 60% /");
    shell
}

#[test]
fn collector_uses_socket_and_only_marks_proven_idle_shells() {
    let server = Server::start(|method, params| match method {
        "agent.list" => json!({"result":{"agents":[{"pane_id":"worker","agent":"pi"}]}}),
        "pane.list" => json!({"result":{"panes":[
            {"pane_id":"worker","agent":"pi"}, {"pane_id":"idle"}, {"pane_id":"gone"}, {"pane_id":"unknown"}
        ]}}),
        "pane.process_info" => match params["pane_id"].as_str() {
            Some("idle") => json!({"result":{"process_info":{
                "shell_pid":100,"foreground_processes":[{"pid":100,"name":"bash","argv":["bash"]}]
            }}}),
            Some("gone") => json!({"error":{"code":"pane_not_found","message":"pane closed"}}),
            Some("unknown") => json!({"error":{"code":"internal","message":"unreadable"}}),
            params => panic!("unexpected process_info for {params:?}"),
        },
        method => panic!("unexpected method {method}"),
    });
    let ledger = Ledger::temporary();
    let shell = capacity();
    let policy = Policy::default();
    let node = Node::new(&shell, &ledger, &policy, "/home");
    let mut collector = Collector::with_socket(server.client());
    let collection = collector.collect(&node, 100);
    collection.sample.unwrap();
    let terminals = &collection.input.local["terminals"];
    assert_eq!(terminals["agents"][0]["pane"], "worker");
    assert_eq!(terminals["panes"][1]["idle_shell"], true);
    for index in [0, 2, 3] {
        assert!(terminals["panes"][index].get("idle_shell").is_none());
    }
    assert_eq!(
        server.methods(),
        [
            "agent.list",
            "pane.list",
            "pane.process_info",
            "pane.process_info",
            "pane.process_info"
        ]
    );
    assert!(
        shell
            .calls
            .lock()
            .unwrap()
            .iter()
            .all(|call| !call.starts_with("herdr ") && !call.starts_with("iostat "))
    );
    let reused = collector.refresh(&node, 200, false);
    reused.sample.unwrap();
    assert_eq!(reused.input.sources.local["terminals"], 100);
    assert_eq!(server.methods().len(), 5);
}

#[test]
fn failed_socket_sample_keeps_last_terminal_observation() {
    let malformed = Arc::new(AtomicBool::new(false));
    let server = Server::start({
        let malformed = Arc::clone(&malformed);
        move |method, _| match method {
            "agent.list" if malformed.load(Ordering::SeqCst) => json!({"result":{}}),
            "agent.list" => json!({"result":{"agents":[]}}),
            "pane.list" => json!({"result":{"panes":[]}}),
            method => panic!("unexpected method {method}"),
        }
    });
    let ledger = Ledger::temporary();
    // Exercise the capacity path without /proc files. An iostat call has no fixture
    // and would fail the first sample instead of publishing it immediately.
    let shell = Fake::new();
    shell
        .on("sysctl -n vm.loadavg", "{ 0.5 0.2 0.1 }")
        .on("sysctl -n hw.memsize", "65536")
        .on("sysctl -n kern.memorystatus_vm_pressure_level", "1")
        .on(
            "vm_stat",
            "Mach Virtual Memory Statistics: (page size of 4096 bytes)\nPages free: 8.\nPages inactive: 1.\nPages speculative: 0.\n",
        )
        .on("df -Pk /home", "FS 1000 500 400 60% /");
    let policy = Policy::default();
    let node = Node::new(&shell, &ledger, &policy, "/home");
    let mut collector = Collector::with_socket(server.client());
    let original = collector.collect(&node, 100);
    original.sample.unwrap();
    malformed.store(true, Ordering::SeqCst);
    let failed = collector.collect(&node, 200);
    assert!(failed.sample.is_err());
    assert_eq!(
        failed.input.local["terminals"],
        original.input.local["terminals"]
    );
    assert_eq!(failed.input.sources.local["terminals"], 100);
    assert!(
        failed
            .input
            .gaps
            .iter()
            .any(|gap| gap.domain == "terminals")
    );
    assert_eq!(server.methods(), ["agent.list", "pane.list", "agent.list"]);
}
