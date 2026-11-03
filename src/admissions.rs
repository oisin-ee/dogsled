//! Admission attempts in `state.db`: one typed row per request, with child tables for
//! lists. Peer protocol messages are kept as the exact bytes exchanged.

use rusqlite::types::Value as Sql;
use rusqlite::{OptionalExtension, Row, Transaction, params};
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::error::{Error, Result};
use crate::ledger::Ledger;
use crate::rows::{
    self, Columns, decode, decoded, diagnostic_sql, integer, list, number, request_sql, required,
    text,
};
use crate::store;
use crate::unit_request::{Prepared, ReferenceReleaseIntent};
use crate::unit_transport::{ExecutionRef, Placement, Review, UnitRef};
use crate::work::{Attempt, Request};

pub(crate) const SCHEMA: &str = concat!(
    "
CREATE TABLE admissions (
    id TEXT PRIMARY KEY,",
    request_sql!(),
    "
    finished_ms INTEGER,
    host_id TEXT,
    host_label TEXT,
    host_target TEXT,
    error TEXT,",
    diagnostic_sql!(),
    "
    launch_message BLOB,
    result_message BLOB,
    ordinary_release_certificate BLOB,
    ordinary_release_acknowledgement BLOB,
    ordinary_release_result TEXT,
    placement_version INTEGER,
    placement_completion_ack INTEGER,
    placement_phase TEXT,
    placement_phase_message BLOB,
    unit_session_id TEXT,
    unit_unit_id TEXT,
    unit_revision INTEGER,
    unit_contract_entry_id TEXT,
    unit_executor TEXT,
    unit_owner_id TEXT,
    unit_request_id TEXT,
    unit_host TEXT,
    unit_branch TEXT,
    unit_base TEXT,
    unit_tool_call_id TEXT,
    unit_committed BLOB,
    unit_abandoned BLOB,
    unit_returned BLOB,
    unit_terminal_receipt BLOB,
    unit_stopped BLOB,
    unit_settlement_ack TEXT,
    unit_release_certificate BLOB,
    unit_release_acknowledgement BLOB,
    unit_release_reason TEXT,
    CHECK ((unit_owner_id IS NULL) = (unit_tool_call_id IS NULL)),
    CHECK ((placement_version IS NULL) = (placement_phase IS NULL)),
    CHECK ((diagnostic_version IS NULL) = (diagnostic_operation IS NULL))
);
CREATE INDEX admissions_owner ON admissions (unit_owner_id) WHERE unit_owner_id IS NOT NULL;
CREATE INDEX admissions_unsettled ON admissions (unit_owner_id)
    WHERE unit_owner_id IS NOT NULL AND unit_settlement_ack IS NULL;
CREATE TABLE admission_job_args (
    admission TEXT NOT NULL REFERENCES admissions (id) ON DELETE CASCADE,
    position INTEGER NOT NULL,
    arg TEXT NOT NULL,
    PRIMARY KEY (admission, position)
) WITHOUT ROWID;
CREATE TABLE admission_declined (
    admission TEXT NOT NULL REFERENCES admissions (id) ON DELETE CASCADE,
    position INTEGER NOT NULL,
    reply_message BLOB NOT NULL,
    PRIMARY KEY (admission, position)
) WITHOUT ROWID;
CREATE TABLE admission_custody_acks (
    admission TEXT NOT NULL REFERENCES admissions (id) ON DELETE CASCADE,
    position INTEGER NOT NULL,
    claim_id TEXT NOT NULL,
    PRIMARY KEY (admission, position)
) WITHOUT ROWID;
CREATE TABLE admission_reviews (
    admission TEXT NOT NULL REFERENCES admissions (id) ON DELETE CASCADE,
    position INTEGER NOT NULL,
    claim_id TEXT NOT NULL,
    revision INTEGER NOT NULL,
    executor TEXT NOT NULL,
    owner_id TEXT NOT NULL,
    request_id TEXT NOT NULL,
    result_id TEXT NOT NULL,
    review_id TEXT NOT NULL,
    evidence TEXT NOT NULL,
    reviewer TEXT NOT NULL,
    PRIMARY KEY (admission, position)
) WITHOUT ROWID;
"
);

pub(crate) fn get(ledger: &Ledger, id: &str) -> Result<Option<Attempt>> {
    store::read(ledger, |tx| load(tx, id))
}

/// The attempt in its serialized form, for readers that compare whole records.
pub(crate) fn value(ledger: &Ledger, id: &str) -> Result<Option<Value>> {
    get(ledger, id)?
        .map(|attempt| Ok(serde_json::to_value(attempt)?))
        .transpose()
}

pub(crate) fn get_as<T: DeserializeOwned>(ledger: &Ledger, id: &str) -> Result<Option<T>> {
    value(ledger, id)?
        .map(|value| Ok(serde_json::from_value(value)?))
        .transpose()
}

pub(crate) fn exists(ledger: &Ledger, id: &str) -> Result<bool> {
    store::read(ledger, |tx| {
        Ok(tx
            .query_row("SELECT 1 FROM admissions WHERE id = ?1", [id], |_| Ok(()))
            .optional()?
            .is_some())
    })
}

pub(crate) fn put(ledger: &Ledger, id: &str, attempt: &Attempt) -> Result<()> {
    store::write(ledger, |tx| save(tx, id, attempt))
}

/// Save a serialized attempt. A value without the shape of an [`Attempt`] is refused.
pub(crate) fn put_value(ledger: &Ledger, id: &str, attempt: &Value) -> Result<()> {
    put(ledger, id, &serde_json::from_value(attempt.clone())?)
}

pub(crate) fn remove(ledger: &Ledger, id: &str) -> Result<()> {
    store::write(ledger, |tx| {
        tx.execute("DELETE FROM admissions WHERE id = ?1", [id])?;
        Ok(())
    })
}

pub(crate) fn ids(ledger: &Ledger) -> Result<Vec<String>> {
    store::read(ledger, |tx| {
        let mut statement = tx.prepare("SELECT id FROM admissions ORDER BY rowid")?;
        let ids = statement.query_map([], |row| row.get(0))?;
        Ok(ids.collect::<rusqlite::Result<_>>()?)
    })
}

pub(crate) fn all(ledger: &Ledger) -> Result<Vec<Attempt>> {
    store::read(ledger, |tx| {
        Ok(select(tx, "1 ORDER BY rowid", None)?
            .into_iter()
            .map(|(_, attempt)| attempt)
            .collect())
    })
}

/// Unit attempts committed by `owner` whose settlement it has not acknowledged.
pub(crate) fn unsettled(ledger: &Ledger, owner: &str) -> Result<Vec<Attempt>> {
    store::read(ledger, |tx| {
        Ok(select(
            tx,
            "unit_owner_id = ?1 AND unit_settlement_ack IS NULL ORDER BY rowid",
            Some(owner),
        )?
        .into_iter()
        .map(|(_, attempt)| attempt)
        .collect())
    })
}

/// Unit attempts committed by `owner`, settled or not.
pub(crate) fn owned(ledger: &Ledger, owner: &str) -> Result<Vec<Attempt>> {
    store::read(ledger, |tx| {
        Ok(
            select(tx, "unit_owner_id = ?1 ORDER BY rowid", Some(owner))?
                .into_iter()
                .map(|(_, attempt)| attempt)
                .collect(),
        )
    })
}

/// Hold the request's admission lock while `f` runs.
pub(crate) fn lock<T>(ledger: &Ledger, id: &str, f: impl FnOnce() -> Result<T>) -> Result<T> {
    ledger.child("admissions").with_lock(id, f)
}

/// Every `(request id, attempt)` pair in its serialized form.
pub(crate) fn entries(ledger: &Ledger) -> Result<Vec<(String, Value)>> {
    store::read(ledger, |tx| {
        select(tx, "1 ORDER BY rowid", None)?
            .into_iter()
            .map(|(id, attempt)| Ok((id, serde_json::to_value(attempt)?)))
            .collect()
    })
}

/// Every attempt in its serialized form.
pub(crate) fn values(ledger: &Ledger) -> Result<Vec<Value>> {
    all(ledger)?
        .into_iter()
        .map(|attempt| Ok(serde_json::to_value(attempt)?))
        .collect()
}

fn select(
    tx: &Transaction<'_>,
    filter: &str,
    value: Option<&str>,
) -> Result<Vec<(String, Attempt)>> {
    let mut statement = tx.prepare(&format!("SELECT * FROM admissions WHERE {filter}"))?;
    let mut rows = statement.query(rusqlite::params_from_iter(value))?;
    let mut attempts = Vec::new();
    while let Some(row) = rows.next()? {
        let id: String = row.get("id")?;
        let attempt = attempt(tx, &id, row)?;
        attempts.push((id, attempt));
    }
    Ok(attempts)
}

fn load(tx: &Transaction<'_>, id: &str) -> Result<Option<Attempt>> {
    let mut statement = tx.prepare_cached("SELECT * FROM admissions WHERE id = ?1")?;
    let mut rows = statement.query([id])?;
    match rows.next()? {
        Some(row) => Ok(Some(attempt(tx, id, row)?)),
        None => Ok(None),
    }
}

fn save(tx: &Transaction<'_>, id: &str, attempt: &Attempt) -> Result<()> {
    let request: Request = serde_json::from_value(attempt.request.clone())?;
    let mut columns = Columns::default();
    columns.text("id", Some(id));
    let args = rows::request_columns(&mut columns, &request)?;
    columns.number("finished_ms", attempt.finished_ms)?;
    if let Some(host) = &attempt.host {
        columns.text("host_id", Some(&host.id));
        columns.text("host_label", Some(&host.label));
        columns.text("host_target", host.target.as_deref());
    }
    columns.text("error", attempt.error.as_deref());
    rows::diagnostic_columns(&mut columns, attempt.diagnostic.as_ref())?;
    columns.message("launch_message", attempt.launch.as_ref())?;
    columns.message("result_message", attempt.result.as_ref())?;
    if let Some(release) = &attempt.ordinary_release {
        columns.message("ordinary_release_certificate", Some(&release.certificate))?;
        columns.message(
            "ordinary_release_acknowledgement",
            release.acknowledgement.as_ref(),
        )?;
        columns.text("ordinary_release_result", Some(&release.result));
    }
    let declined = match &attempt.placement {
        Some(progress) => placement_columns(&mut columns, progress)?,
        None => Vec::new(),
    };
    if let Some(unit) = &attempt.unit {
        unit_columns(&mut columns, unit)?;
    }
    columns.upsert(tx, "admissions", "id")?;
    rows::replace_list(
        tx,
        "admission_job_args",
        "admission",
        "arg",
        id,
        args.into_iter().map(Sql::Text),
    )?;
    rows::replace_list(
        tx,
        "admission_declined",
        "admission",
        "reply_message",
        id,
        declined,
    )?;
    let (acks, reviews): (&[String], &[Review]) = attempt
        .unit
        .as_ref()
        .map_or((&[], &[]), |unit| (&unit.custody_acks, &unit.reviews));
    rows::replace_list(
        tx,
        "admission_custody_acks",
        "admission",
        "claim_id",
        id,
        acks.iter().cloned().map(Sql::Text),
    )?;
    save_reviews(tx, id, reviews)
}

/// The placement columns, and the declined replies for the child table.
fn placement_columns(
    columns: &mut Columns,
    progress: &crate::work::PlacementProgress,
) -> Result<Vec<Sql>> {
    columns.number("placement_version", Some(u64::from(progress.version)))?;
    columns.put(
        "placement_completion_ack",
        progress
            .completion_ack
            .map_or(Sql::Null, |ack| Sql::Integer(i64::from(ack))),
    );
    let Value::Object(mut phase) = serde_json::to_value(&progress.phase)? else {
        return Err(Error::Invalid("placement phase is not an object".into()));
    };
    let Some(Value::String(tag)) = phase.remove("phase") else {
        return Err(Error::Invalid("placement phase has no name".into()));
    };
    columns.text("placement_phase", Some(&tag));
    columns.message("placement_phase_message", Some(&Value::Object(phase)))?;
    progress
        .declined
        .iter()
        .map(|reply| Ok(Sql::Blob(rows::message(reply)?)))
        .collect()
}

fn unit_columns(columns: &mut Columns, unit: &Prepared) -> Result<()> {
    columns.text("unit_session_id", Some(&unit.origin.session_id));
    columns.text("unit_unit_id", Some(&unit.origin.unit_id));
    columns.number("unit_revision", Some(u64::from(unit.origin.revision)))?;
    columns.text(
        "unit_contract_entry_id",
        Some(&unit.origin.contract_entry_id),
    );
    columns.text("unit_executor", Some(&unit.execution.executor));
    columns.text("unit_owner_id", Some(&unit.execution.owner_id));
    columns.text("unit_request_id", Some(&unit.execution.request_id));
    columns.text("unit_host", unit.placement.host.as_deref());
    columns.text("unit_branch", unit.placement.branch.as_deref());
    columns.text("unit_base", unit.placement.base.as_deref());
    columns.text("unit_tool_call_id", Some(&unit.tool_call_id));
    columns.message("unit_committed", unit.committed.as_ref())?;
    columns.message("unit_abandoned", unit.abandoned.as_ref())?;
    columns.message("unit_returned", unit.returned.as_ref())?;
    columns.message("unit_terminal_receipt", unit.terminal_receipt.as_ref())?;
    columns.message("unit_stopped", unit.stopped.as_ref())?;
    columns.text("unit_settlement_ack", unit.settlement_ack.as_deref());
    if let Some(release) = &unit.reference_release {
        columns.message("unit_release_certificate", Some(&release.certificate))?;
        columns.message(
            "unit_release_acknowledgement",
            release.acknowledgement.as_ref(),
        )?;
        columns.text("unit_release_reason", release.reason.as_deref());
    }
    Ok(())
}

fn save_reviews(tx: &Transaction<'_>, id: &str, reviews: &[Review]) -> Result<()> {
    tx.execute("DELETE FROM admission_reviews WHERE admission = ?1", [id])?;
    for (position, review) in (0_i64..).zip(reviews.iter()) {
        tx.prepare_cached(
            "INSERT INTO admission_reviews (admission, position, claim_id, revision, executor,
                 owner_id, request_id, result_id, review_id, evidence, reviewer)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        )?
        .execute(params![
            id,
            position,
            review.claim_id,
            integer(review.revision)?,
            review.execution.executor,
            review.execution.owner_id,
            review.execution.request_id,
            review.result_id,
            review.review_id,
            review.evidence,
            review.reviewer,
        ])?;
    }
    Ok(())
}

fn attempt(tx: &Transaction<'_>, id: &str, row: &Row<'_>) -> Result<Attempt> {
    let request = rows::load_request(row, || {
        list(
            tx,
            "SELECT arg FROM admission_job_args WHERE admission = ?1 ORDER BY position",
            id,
            |row| row.get(0),
        )
    })?;
    let host = match text(row, "host_id")? {
        Some(host_id) => Some(crate::hosts::Host {
            id: host_id,
            label: required(row, "host_label")?,
            target: text(row, "host_target")?,
        }),
        None => None,
    };
    let ordinary_release = match decoded(row, "ordinary_release_certificate")? {
        Some(certificate) => Some(crate::launch::OrdinaryReleaseIntent {
            certificate,
            acknowledgement: decoded(row, "ordinary_release_acknowledgement")?,
            result: required(row, "ordinary_release_result")?,
        }),
        None => None,
    };
    Ok(Attempt {
        request: serde_json::to_value(request)?,
        finished_ms: number(row, "finished_ms")?,
        ordinary_release,
        placement: load_placement(tx, id, row)?,
        host,
        launch: decoded(row, "launch_message")?,
        error: text(row, "error")?,
        diagnostic: rows::load_diagnostic(row)?,
        unit: load_unit(tx, id, row)?,
        result: decoded(row, "result_message")?,
    })
}

fn load_placement(
    tx: &Transaction<'_>,
    id: &str,
    row: &Row<'_>,
) -> Result<Option<crate::work::PlacementProgress>> {
    let Some(version) = number(row, "placement_version")? else {
        return Ok(None);
    };
    let mut phase: serde_json::Map<String, Value> =
        decoded(row, "placement_phase_message")?.unwrap_or_default();
    phase.insert(
        "phase".into(),
        Value::String(required(row, "placement_phase")?),
    );
    Ok(Some(crate::work::PlacementProgress {
        version: u32::try_from(version)
            .map_err(|_| Error::Invalid(format!("placement version {version}")))?,
        completion_ack: number(row, "placement_completion_ack")?.map(|ack| ack == 1),
        phase: serde_json::from_value(Value::Object(phase))?,
        declined: list(
            tx,
            "SELECT reply_message FROM admission_declined WHERE admission = ?1 ORDER BY position",
            id,
            |row| row.get::<_, Vec<u8>>(0),
        )?
        .iter()
        .map(|reply| decode(reply))
        .collect::<Result<_>>()?,
    }))
}

fn load_unit(tx: &Transaction<'_>, id: &str, row: &Row<'_>) -> Result<Option<Prepared>> {
    let Some(tool_call_id) = text(row, "unit_tool_call_id")? else {
        return Ok(None);
    };
    Ok(Some(Prepared {
        origin: UnitRef {
            session_id: required(row, "unit_session_id")?,
            unit_id: required(row, "unit_unit_id")?,
            revision: u32::try_from(number(row, "unit_revision")?.unwrap_or_default())
                .map_err(|_| Error::Invalid("unit revision".into()))?,
            contract_entry_id: required(row, "unit_contract_entry_id")?,
        },
        execution: ExecutionRef {
            executor: required(row, "unit_executor")?,
            owner_id: required(row, "unit_owner_id")?,
            request_id: required(row, "unit_request_id")?,
        },
        placement: Placement {
            host: text(row, "unit_host")?,
            branch: text(row, "unit_branch")?,
            base: text(row, "unit_base")?,
        },
        tool_call_id,
        committed: decoded(row, "unit_committed")?,
        abandoned: decoded(row, "unit_abandoned")?,
        returned: decoded(row, "unit_returned")?,
        terminal_receipt: decoded(row, "unit_terminal_receipt")?,
        settlement_ack: text(row, "unit_settlement_ack")?,
        custody_acks: list(
            tx,
            "SELECT claim_id FROM admission_custody_acks WHERE admission = ?1 ORDER BY position",
            id,
            |row| row.get(0),
        )?,
        reviews: load_reviews(tx, id)?,
        stopped: decoded(row, "unit_stopped")?,
        reference_release: match decoded(row, "unit_release_certificate")? {
            Some(certificate) => Some(ReferenceReleaseIntent {
                certificate,
                acknowledgement: decoded(row, "unit_release_acknowledgement")?,
                reason: text(row, "unit_release_reason")?,
            }),
            None => None,
        },
    }))
}

fn load_reviews(tx: &Transaction<'_>, id: &str) -> Result<Vec<Review>> {
    list(
        tx,
        "SELECT claim_id, revision, executor, owner_id, request_id, result_id, review_id,
                evidence, reviewer
         FROM admission_reviews WHERE admission = ?1 ORDER BY position",
        id,
        |row| {
            Ok(Review {
                claim_id: row.get(0)?,
                revision: row.get::<_, i64>(1)?.cast_unsigned(),
                execution: ExecutionRef {
                    executor: row.get(2)?,
                    owner_id: row.get(3)?,
                    request_id: row.get(4)?,
                },
                result_id: row.get(5)?,
                review_id: row.get(6)?,
                evidence: row.get(7)?,
                reviewer: row.get(8)?,
            })
        },
    )
}

/// Complete attempts for tests that exercise readers of stored admissions.
#[cfg(test)]
pub(crate) mod fixtures {
    use crate::unit_request::{Prepared, ReferenceReleaseIntent};
    use crate::unit_transport::{Binding, ExecutionRef, Placement, UnitRef};
    use crate::work::{Action, Attempt, Request};

    pub(crate) fn request(id: &str, owner: &str, owner_host: Option<&str>) -> Request {
        Request {
            id: id.into(),
            validity: None,
            action: Action::Place,
            host: None,
            branch: None,
            base: None,
            cwd: "/".into(),
            repository: None,
            workspace: None,
            workspace_name: None,
            owner: owner.into(),
            owner_host: owner_host.map(Into::into),
        }
    }

    pub(crate) fn attempt(request: &Request) -> Attempt {
        Attempt {
            request: serde_json::to_value(request).unwrap(),
            finished_ms: None,
            ordinary_release: None,
            placement: None,
            host: None,
            launch: None,
            error: None,
            diagnostic: None,
            unit: None,
            result: None,
        }
    }

    pub(crate) fn unit(id: &str, owner: &str) -> Prepared {
        Prepared {
            origin: UnitRef {
                session_id: "session-1".into(),
                unit_id: "u-1".into(),
                revision: 1,
                contract_entry_id: "entry-1".into(),
            },
            execution: ExecutionRef {
                executor: "dogsled".into(),
                owner_id: owner.into(),
                request_id: id.into(),
            },
            placement: Placement {
                host: None,
                branch: None,
                base: None,
            },
            tool_call_id: "call-1".into(),
            committed: None,
            abandoned: None,
            returned: None,
            terminal_receipt: None,
            settlement_ack: None,
            custody_acks: Vec::new(),
            reviews: Vec::new(),
            stopped: None,
            reference_release: None,
        }
    }

    /// A unit attempt whose origin saved a reference release the host has not acknowledged.
    pub(crate) fn released(id: &str, owner: &str, owner_host: &str) -> Attempt {
        let request = request(id, owner, Some(owner_host));
        let mut unit = unit(id, owner);
        let binding = Binding {
            contract: unit.origin.clone(),
            execution: unit.execution.clone(),
            export_digest: "0".repeat(64),
        };
        let receipt = crate::unit_reply::Receipt {
            ack: crate::unit_transport::TerminalAck {
                execution: unit.execution.clone(),
                unit_ref: unit.origin.clone(),
                digest: "0".repeat(64),
                seal_entry_id: "seal".into(),
                session_leaf_id: "leaf".into(),
            },
            session_id: "session-1".into(),
            reply_entry_id: "reply".into(),
            text: "FINAL".into(),
        };
        let launch = crate::work::Launch {
            id: id.into(),
            validity: None,
            preparation_protocol: None,
            preparation: None,
            unit: None,
            action: Action::Place,
            host: "worker".into(),
            cwd: "/".into(),
            pane: Some("w2:p1".into()),
            terminal: None,
            workspace: None,
            workspace_name: None,
            owner: owner.into(),
            owner_host: Some(owner_host.into()),
        };
        unit.reference_release = Some(ReferenceReleaseIntent {
            certificate: crate::launch::ReferenceRelease {
                version: 2,
                origin_host: owner_host.into(),
                execution: unit.execution.clone(),
                binding,
                launch,
                terminal_evidence: crate::unit_reply::HostEvidence::Returned(Box::new(receipt)),
                custody_revision: 0,
                released_references: [crate::launch::ReleasedReference::Execution].into(),
            },
            acknowledgement: None,
            reason: None,
        });
        Attempt {
            unit: Some(unit),
            result: Some(serde_json::json!({})),
            ..attempt(&request)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::work::Action;

    fn ledger() -> Ledger {
        Ledger::new(std::env::temp_dir().join(crate::ledger::new_id()))
    }

    #[test]
    fn every_field_survives_a_round_trip_and_an_update_clears_children() {
        let ledger = ledger();
        let mut attempt = fixtures::released("r-1", "w1:p1", "origin");
        let mut request = fixtures::request("r-1", "w1:p1", Some("origin"));
        request.action = Action::Job {
            cmd: vec!["sh".into(), "-c".into(), "true".into()],
            timeout_ms: Some(5),
            idle_timeout_ms: None,
            validity: Some(crate::jobs::Validity::new(7)),
        };
        request.validity = Some(crate::jobs::Validity::new(7));
        attempt.request = serde_json::to_value(&request).unwrap();
        attempt.error = Some("refused".into());
        attempt.finished_ms = Some(9);
        let unit = attempt.unit.as_mut().unwrap();
        unit.custody_acks = vec!["claim-1".into(), "claim-2".into()];
        unit.settlement_ack = Some("event".into());
        put(&ledger, "r-1", &attempt).unwrap();
        let saved = value(&ledger, "r-1").unwrap().unwrap();
        assert_eq!(saved, serde_json::to_value(&attempt).unwrap());
        // A settled unit leaves the caller's unsettled set; others never see it.
        assert!(unsettled(&ledger, "w1:p1").unwrap().is_empty());
        assert_eq!(owned(&ledger, "w1:p1").unwrap().len(), 1);
        assert!(owned(&ledger, "w9:p9").unwrap().is_empty());
        // An update replaces child rows rather than adding to them.
        attempt.unit.as_mut().unwrap().custody_acks = vec!["claim-3".into()];
        attempt.unit.as_mut().unwrap().settlement_ack = None;
        put(&ledger, "r-1", &attempt).unwrap();
        assert_eq!(
            value(&ledger, "r-1").unwrap().unwrap(),
            serde_json::to_value(&attempt).unwrap()
        );
        assert_eq!(unsettled(&ledger, "w1:p1").unwrap().len(), 1);
        remove(&ledger, "r-1").unwrap();
        assert!(!exists(&ledger, "r-1").unwrap());
        std::fs::remove_dir_all(ledger.dir).unwrap();
    }

    #[test]
    fn a_malformed_attempt_is_refused_and_nothing_is_stored() {
        let ledger = ledger();
        for malformed in [
            serde_json::json!("corrupt"),
            serde_json::json!({"request": "corrupt", "launch": null, "error": null}),
            serde_json::json!({"request": {"id": "r-1"}, "launch": null, "error": null}),
        ] {
            assert!(
                put_value(&ledger, "r-1", &malformed).is_err(),
                "{malformed}"
            );
            assert!(!exists(&ledger, "r-1").unwrap());
        }
        std::fs::remove_dir_all(ledger.dir).ok();
    }
}
