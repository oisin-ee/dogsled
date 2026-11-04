use super::super::{Domains, peers, project};
use super::projection::{Fixture, NOW, host};
use crate::service::wake::{LEDGER, PEERS};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;

fn answer(
    fixture: &Fixture,
    target: &crate::hosts::Host,
    request: &peers::Request,
    export: &peers::Export,
) {
    let args = [
        "observation".into(),
        "--request".into(),
        serde_json::to_string(request).unwrap(),
    ];
    fixture.shell.on(
        &format!(
            "ssh {}",
            fixture.fleet().remote_argv(target, &args).join(" ")
        ),
        &serde_json::to_string(export).unwrap(),
    );
}

fn retained_branch(fixture: &Fixture) -> crate::done::Branch {
    let branch = crate::done::Branch {
        repo: "/peer-only/repository".into(),
        name: "retained".into(),
        ahead: 1,
        landed: false,
    };
    crate::store::write(&fixture.ledger, |tx| {
        let mut items = crate::tables::items(tx)?;
        let item = items
            .iter_mut()
            .find(|item| item.id == "peer-child")
            .unwrap();
        item.closed_ms = Some(2);
        item.branch = Some(branch.clone());
        crate::tables::sync_items(tx, &items)
    })
    .unwrap();
    branch
}

fn export(report: &crate::hosts::HostReport, branch: crate::done::Branch) -> peers::Export {
    peers::Export {
        version: super::super::VERSION,
        epoch: "peer-epoch".into(),
        local_revision: 1,
        measured: BTreeMap::from([("host".into(), NOW)]),
        local: Domains::from([
            (
                "host".into(),
                json!({"sampled_ms":NOW,"running":true,"probe":report.probe,"jobs":[]}),
            ),
            (
                "branches".into(),
                serde_json::to_value([peers::BranchObservation {
                    checked: crate::drop::Checked {
                        item: "peer-child".into(),
                        branch: crate::done::Branch {
                            ahead: 0,
                            landed: true,
                            ..branch
                        },
                    },
                    measured_ms: NOW + 1,
                }])
                .unwrap(),
            ),
        ]),
        gaps: vec![],
    }
}

#[test]
fn changed_branch_import_recaptures_local_inputs_but_identical_import_does_not() {
    let fixture = Fixture::new();
    let branch = retained_branch(&fixture);
    let mut input = fixture.input();
    let mut projector = project::Projector::default();
    let old = projector.inputs(&fixture.fleet(), &input);
    let report = host("peer", "working");
    project::derive(
        &old,
        vec![fixture.local.clone(), report.clone()],
        &(&fixture.policy).into(),
        &mut input,
    )
    .unwrap();
    let retained = |input: &super::super::Publication| {
        input.views["boards"]["data"]["owner"]["rows"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["id"] == "peer-child")
    };
    assert!(retained(&input));
    let export = export(&report, branch);
    let mut ingestion = peers::Ingestion::default();
    answer(&fixture, &report.host, &peers::Request::default(), &export);
    crate::store::observing(|| {
        ingestion.refresh(&fixture.fleet(), &report.host, NOW + 1, vec![], false)
    })
    .unwrap();
    let domains = PEERS | ingestion.take_local_changes();
    assert_eq!(
        domains,
        PEERS | LEDGER,
        "import is the only local invalidation"
    );
    input = fixture.input();
    let next = projector.inputs(&fixture.fleet(), &input);
    assert!(!Arc::ptr_eq(&old, &next));
    project::derive(
        &next,
        vec![fixture.local.clone(), report.clone()],
        &(&fixture.policy).into(),
        &mut input,
    )
    .unwrap();
    assert!(!retained(&input));
    let item = input.views["status"]["data"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == "peer-child")
        .unwrap();
    assert_eq!(item["branch"]["landed"], true);
    assert_eq!(item["branch_checked_ms"], NOW + 1);
    let request = peers::Request {
        epoch: Some(export.epoch.clone()),
        local_revision: Some(1),
        ..peers::Request::default()
    };
    answer(&fixture, &report.host, &request, &export);
    crate::store::observing(|| {
        ingestion.refresh(&fixture.fleet(), &report.host, NOW + 10_001, vec![], false)
    })
    .unwrap();
    assert_eq!(
        ingestion.take_local_changes(),
        0,
        "identical import cannot trigger LEDGER"
    );
    // Feed the same captured collector input back, without project-added source metadata.
    let mut unchanged = fixture.input();
    let reused = projector.inputs(&fixture.fleet(), &unchanged);
    assert!(
        Arc::ptr_eq(&next, &reused),
        "unchanged input must not recapture"
    );
    project::derive(
        &reused,
        vec![fixture.local.clone(), report],
        &(&fixture.policy).into(),
        &mut unchanged,
    )
    .unwrap();
    assert_eq!(input.views["status"], unchanged.views["status"]);
    assert_eq!(input.views["boards"], unchanged.views["boards"]);
    assert_ne!(input.views["status"]["data"]["items"], Value::Null);
}

#[test]
fn identical_unlanded_import_reports_no_local_row_change() {
    let fixture = Fixture::new();
    let branch = crate::done::Branch {
        repo: "/peer-only/repository".into(),
        name: "retained".into(),
        ahead: 1,
        landed: false,
    };
    crate::store::write(&fixture.ledger, |tx| {
        let mut items = crate::tables::items(tx)?;
        let item = items
            .iter_mut()
            .find(|item| item.id == "peer-child")
            .unwrap();
        item.closed_ms = Some(2);
        item.branch = Some(branch.clone());
        crate::tables::sync_items(tx, &items)
    })
    .unwrap();
    let result = peers::BranchObservation {
        checked: crate::drop::Checked {
            item: "peer-child".into(),
            branch,
        },
        measured_ms: NOW,
    };
    assert!(
        crate::store::observing(|| peers::import(
            &fixture.ledger,
            "peer",
            std::slice::from_ref(&result)
        ))
        .unwrap()
    );
    assert!(
        !crate::store::observing(|| peers::import(
            &fixture.ledger,
            "peer",
            std::slice::from_ref(&result)
        ))
        .unwrap()
    );
}
