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

fn lead(id: &str, pane: Option<&str>, validity: crate::jobs::Validity) -> crate::leads::Record {
    crate::leads::Record {
        id: id.into(),
        name: format!("lead-{id}"),
        repository: crate::checkout::Repository {
            path: "/home/repo".into(),
            origin: "https://github.com/oisin-ee/dogsled".into(),
            relative: "repo".into(),
            working_root: "/home/repo".into(),
        },
        prompt: "go".into(),
        validity,
        created_ms: validity.created_ms,
        phase: crate::leads::Phase::Ready,
        pane: pane.map(str::to_owned),
        terminal: Some("term".into()),
        workspace: None,
        updated_ms: validity.created_ms,
        error: None,
        close_requested_ms: None,
        closed_ms: None,
    }
}

/// A lead is ownerless, so its validity belongs to the lead's own pane. Without that, every
/// open lead published a permanent gap and the portal reported live updates as paused.
#[test]
fn lead_validity_is_owned_by_its_pane_and_publishes_no_gap() {
    let root = std::env::temp_dir().join(crate::ledger::new_id());
    let ledger = Ledger::new(root.clone());
    let shell = shell();
    let policy = crate::config::Policy::default();
    let node = crate::node::Node::new(&shell, &ledger, &policy, "/home");
    let validity = crate::jobs::issue_unowned_validity(&ledger, "lead-request").unwrap();
    let now = validity.created_ms;
    let mut collector = collect::Collector::with_scripted_shell();
    let allocating = collector.collect(&node, now);
    allocating.sample.unwrap();
    // Without a matching lead record, validity still has no proved owner.
    assert_eq!(
        allocating
            .input
            .gaps
            .iter()
            .map(|gap| gap.domain.as_str())
            .collect::<Vec<_>>(),
        ["holds/job-validity/lead-request"]
    );

    // An exact lead record proves that the absent parent is intentional, even
    // during allocation. Keep its validity without inventing a pane or releasing it.
    let mut recorded = lead("lead-request", None, validity);
    recorded.phase = crate::leads::Phase::Recorded;
    crate::lead_store::insert(&ledger, &recorded).unwrap();
    let recorded = collector.collect(&node, now + 1);
    recorded.sample.unwrap();
    assert_eq!(recorded.input.gaps, []);
    let retained = &recorded.input.local["host"]["holds"][0];
    assert_eq!(retained["code"], "lead_validity");
    assert_eq!(retained["resource"], "job-validity/lead-request");
    assert!(retained["owner_pane"].is_null());
    assert_eq!(
        crate::jobs::request_validity(&ledger, "lead-request").unwrap(),
        Some(validity)
    );

    crate::lead_store::update(&ledger, "lead-request", |saved| {
        saved.pane = Some("w1:p1".into());
        saved.phase = crate::leads::Phase::Ready;
        Ok(())
    })
    .unwrap();
    let ready = collector.collect(&node, now + 2);
    ready.sample.unwrap();
    assert_eq!(ready.input.gaps, []);
    let holds: Vec<crate::holds::Hold> =
        serde_json::from_value(ready.input.local["host"]["holds"].clone()).unwrap();
    assert_eq!(holds.len(), 1);
    assert_eq!(holds[0].resource, "job-validity/lead-request");
    assert_eq!(holds[0].code, "validity");
    assert_eq!(holds[0].owner_pane.as_deref(), Some("w1:p1"));
    std::fs::remove_dir_all(root).unwrap();
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
    let mut collector = collect::Collector::with_scripted_shell();
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
