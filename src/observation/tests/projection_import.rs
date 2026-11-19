use super::super::{Domains, peers, project};
use super::projection::{Fixture, NOW, host};
use crate::service::wake::{LEDGER, PEERS};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;

#[test]
fn owned_work_projection_has_the_same_direct_owner_on_both_viewers() {
    use super::super::peers::Ownership;
    let owned = json!({"request_id":"dev","kind":"job","owner_pane":"w2DT:pD","phase":"admitted","pane":"w18W:p2","name":"bash"});
    let exec = json!({"request_id":"dev","kind":"job","owner_hostname":"netcup","owner_pane":"w2DT:pD","pane":"w18W:p2","title":"bash","job_label":"bash · web"});
    let evidence = |hostname, owned, executions| json!({"owned_work":{"version":1,"status":"ready","hostname":hostname,"owned":owned,"executions":executions}});
    for origin_view in [true, false] {
        let fixture = Fixture::new();
        let (origin, mac) = if origin_view {
            ("local", "mac")
        } else {
            ("netcup", "local")
        };
        if origin_view {
            crate::items::record(
                &fixture.ledger,
                crate::items::Item {
                    id: "dev".into(),
                    host: mac.into(),
                    pane: "w18W:p2".into(),
                    kind: "job".into(),
                    owner_pane: "w2DT:pD".into(),
                    ..Default::default()
                },
            )
            .unwrap();
        }
        let net_data = evidence("netcup", json!([owned]), json!([]));
        let mac_data = evidence("mac", json!([]), json!([exec]));
        let mut input = fixture.input();
        input.local.insert(
            "host".into(),
            if origin_view {
                net_data.clone()
            } else {
                mac_data.clone()
            },
        );
        let mut projector = project::Projector::default();
        let local = projector.inputs(&fixture.fleet(), &input);
        let mut mac_report = host(mac, "working");
        mac_report.jobs.push(serde_json::from_value(json!({"id":"dev","cmd":["bash"],"cwd":"/work","host":"local","owner":"w2DT:pD","pane":"w18W:p2","state":"working"})).unwrap());
        let mut collision = host("other", "working");
        collision.probe.as_mut().unwrap().agent_panes[0].pane = "w18W:p2".into();
        let reports = vec![host(origin, "working"), mac_report, collision];
        let peer_key = if origin_view { mac } else { origin };
        let mut peer = Ownership::default();
        for step in 0..3 {
            match step {
                0 => peer.replace(Some(if origin_view { &mac_data } else { &net_data }), true),
                1 => peer.replace(None, false),
                _ => peer.replace(Some(&evidence(peer_key, json!([]), json!([]))), true),
            }
            project::derive_with_ownership(
                &local,
                reports.clone(),
                &(&fixture.policy).into(),
                &mut input,
                BTreeMap::from([(peer_key.to_string(), peer.clone())]),
            )
            .unwrap();
            let rows: Vec<_> = input.views["portal_board"]["data"]["repos"]
                .as_array()
                .unwrap()
                .iter()
                .flat_map(|r| r["rows"].as_array().unwrap())
                .filter(|r| r["id"] == "dev")
                .collect();
            assert_eq!(rows.len(), usize::from(origin_view || step != 2));
            if step < 2 {
                let hosts = input.views["portal_board"]["data"]["hosts"]
                    .as_array()
                    .unwrap();
                assert_eq!(
                    hosts.iter().find(|h| h["label"] == origin).unwrap()["hostname"],
                    "netcup"
                );
                assert_eq!(rows[0]["host"], mac);
                assert_eq!(rows[0]["pane"], "w18W:p2");
                assert_eq!(rows[0]["owner_host"], origin);
                assert_eq!(rows[0]["owner_pane"], "w2DT:pD");
                assert_eq!(
                    rows[0]["ownership_stale"].as_bool().unwrap_or(false),
                    step == 1
                );
            }
            assert_eq!(
                input.views["board"]["data"]["rows"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|r| r["id"] == "dev"),
                origin_view
            );
        }
    }
}

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
