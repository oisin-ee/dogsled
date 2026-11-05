use super::super::*;
use crate::ledger::Ledger;

fn shell() -> crate::shell::Fake {
    let shell = crate::shell::Fake::new();
    shell
        .on("herdr agent list", r#"{"result":{"agents":[]}}"#)
        .on("herdr pane list", r#"{"result":{"panes":[]}}"#)
        .on(
            "read /proc/meminfo",
            "MemTotal: 100 kB\nMemAvailable: 75 kB",
        )
        .on("read /proc/loadavg", "0.5 0.2 0.1 1/10 42")
        .on("read /proc/stat", "cpu 100 20 30 400 50 6 7 8 9 10")
        .on("df -Pk /home", "FS 1000 500 400 60% /");
    shell
}

#[test]
fn ownerless_ingress_hold_is_counted_with_a_gap_while_host_facts_stay_fresh() {
    let root = std::env::temp_dir().join(crate::ledger::new_id());
    let ledger = Ledger::new(root.clone());
    let shell = shell();
    let policy = crate::config::Policy::default();
    let node = crate::node::Node::new(&shell, &ledger, &policy, "/home");
    let validity = crate::jobs::origin_validity(&ledger).unwrap();
    let now = validity.created_ms;
    let mut collector = collect::Collector::default();
    collector.collect(&node, now).sample.unwrap();

    // Real ingress commits authority before Node::reserve records its owner.
    crate::jobs::receive_request_validity(&ledger, "newcomer", Some(validity)).unwrap();
    assert!(crate::job_store::owners(&ledger).unwrap().is_empty());
    assert!(crate::work::queue(&ledger).unwrap().is_empty());
    let captured = collector.collect(&node, now + 1);
    captured.sample.unwrap();
    for domain in ["host", "headroom", "status", "todos", "terminals"] {
        assert_eq!(captured.input.sources.local[domain], now + 1, "{domain}");
    }
    assert_eq!(captured.input.local["host"]["sampled_ms"], now + 1);
    assert_eq!(captured.input.gaps.len(), 1);
    let gap = &captured.input.gaps[0];
    assert_eq!(gap.domain, "holds/job-validity/newcomer");
    assert_eq!(gap.availability, Availability::Failed);
    assert!(gap.detail.contains("job-validity/newcomer"));
    assert!(gap.detail.contains("no recorded owner pane"));
    let holds: Vec<crate::holds::Hold> =
        serde_json::from_value(captured.input.local["host"]["holds"].clone()).unwrap();
    assert_eq!(holds.len(), 1);
    assert_eq!(holds[0].resource, "job-validity/newcomer");
    assert_eq!(holds[0].code, "unreadable");
    assert_eq!(holds[0].owner_pane, None);
    assert_eq!(holds[0].request_id.as_deref(), Some("newcomer"));
    assert_eq!(
        captured.input.local["status"]["holds"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(collector.reports()[0].holds, holds);
    assert_eq!(
        crate::jobs::request_validity(&ledger, "newcomer").unwrap(),
        Some(validity)
    );
    assert!(!crate::work::settled(&ledger, "newcomer", now + 1).unwrap());
    let mut publisher = Publisher::new().unwrap();
    let frame = publisher.replace(captured.input).unwrap();
    assert_eq!(frame.gaps[0].domain, "holds/job-validity/newcomer");
    assert_eq!(frame.local["host"]["holds"].as_array().unwrap().len(), 1);

    // Missing ownership must not erase a persisted admission-capacity hold either.
    let reservation = crate::ledger::Reservation {
        id: "newcomer".into(),
        created_ms: now,
    };
    crate::job_store::put_reservations(&ledger, std::slice::from_ref(&reservation)).unwrap();
    let capacity = collector.collect(&node, now + 2);
    capacity.sample.unwrap();
    assert_eq!(capacity.input.sources.local["host"], now + 2);
    assert_eq!(capacity.input.gaps.len(), 2);
    assert_eq!(
        capacity.input.local["host"]["holds"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        collector.reports()[0].probe.as_ref().unwrap().reservations,
        vec![reservation.clone()]
    );
    assert_eq!(
        ledger
            .reservations(now + 2, policy.reservation_ttl_ms)
            .unwrap(),
        vec![reservation]
    );

    crate::job_store::remember_owner(&ledger, "newcomer", "owner", Some("origin")).unwrap();
    let recovered = collector.collect(&node, now + 3);
    recovered.sample.unwrap();
    assert_eq!(recovered.input.gaps, []);
    assert_eq!(recovered.input.sources.local["host"], now + 3);
    assert_eq!(
        recovered.input.local["host"]["holds"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    for hold in recovered.input.local["host"]["holds"].as_array().unwrap() {
        assert!(matches!(
            hold["code"].as_str(),
            Some("capacity" | "validity")
        ));
        assert_eq!(hold["owner_pane"], "owner");
    }
    std::fs::remove_dir_all(root).unwrap();
}
