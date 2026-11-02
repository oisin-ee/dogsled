//! Host-owned launch state. SQLite rows are authoritative; the Pi seed file is
//! only an outbound transport artifact and is never read by Dogsled.

use crate::error::Result;
use crate::launch::{OrdinaryReleaseAck, PreflightRefusal, Progress, ReferenceReleaseAck};
use crate::ledger::Ledger;
use crate::rows::{self, Columns};
use crate::rows::{decoded, number, required, text};
use crate::store;
use rusqlite::{Row, Transaction};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;

pub(crate) fn progress(ledger: &Ledger, id: &str) -> Result<Option<Progress>> {
    store::read(ledger, |tx| load_progress(tx, id))
}

fn load_progress(tx: &Transaction<'_>, id: &str) -> Result<Option<Progress>> {
    let mut statement = tx.prepare_cached("SELECT * FROM launch_progress WHERE id = ?1")?;
    let mut records = statement.query([id])?;
    records.next()?.map(progress_row).transpose()
}

fn message<T: DeserializeOwned>(row: &Row<'_>, column: &str) -> Result<T> {
    decoded(row, column)?.ok_or_else(|| crate::error::Error::Invalid(format!("missing {column}")))
}

fn progress_row(row: &Row<'_>) -> Result<Progress> {
    let preflight_refusal = match text(row, "preflight_kind")?.as_deref() {
        Some("line_budget") => Some(PreflightRefusal::LineBudget {
            bytes: usize::try_from(rows::unsigned(row.get("preflight_bytes")?)?).map_err(|_| {
                crate::error::Error::Invalid("preflight byte count exceeds usize".into())
            })?,
        }),
        Some("validity") => Some(PreflightRefusal::Validity {
            detail: required(row, "preflight_detail")?,
        }),
        _ => None,
    };
    let origin_release = decoded(row, "origin_release_certificate")?
        .map(|certificate| -> Result<_> {
            Ok(ReferenceReleaseAck {
                certificate,
                accepted_ms: rows::unsigned(row.get("origin_release_accepted_ms")?)?,
            })
        })
        .transpose()?;
    Ok(Progress {
        request: message(row, "request_message")?,
        finished_ms: number(row, "finished_ms")?,
        closed_ms: number(row, "closed_ms")?,
        pane: text(row, "pane")?,
        terminal: text(row, "terminal")?,
        attempted: row.get("attempted")?,
        confirmed: row.get("confirmed")?,
        result: message(row, "result_message")?,
        error: text(row, "error")?,
        preflight_refusal,
        terminal_receipt: decoded(row, "terminal_receipt_message")?,
        stopped: decoded(row, "stopped_message")?,
        origin_release,
    })
}

pub(crate) fn progress_value(ledger: &Ledger, id: &str) -> Result<Option<Value>> {
    progress(ledger, id)?
        .map(|progress| Ok(serde_json::to_value(progress)?))
        .transpose()
}

#[cfg(test)]
pub(crate) fn put_progress_value(
    ledger: &Ledger,
    id: &str,
    progress: &impl Serialize,
) -> Result<()> {
    put_progress(
        ledger,
        id,
        &serde_json::from_value(serde_json::to_value(progress)?)?,
    )
}

pub(crate) fn put_progress(ledger: &Ledger, id: &str, progress: &Progress) -> Result<()> {
    crate::jobs::validate_id(id)?;
    store::write(ledger, |tx| save_progress(tx, id, progress))
}

pub(crate) fn update_progress(
    ledger: &Ledger,
    id: &str,
    update: impl FnOnce(&mut Progress) -> Result<bool>,
) -> Result<()> {
    store::write(ledger, |tx| {
        if let Some(mut progress) = load_progress(tx, id)?
            && update(&mut progress)?
        {
            save_progress(tx, id, &progress)?;
        }
        Ok(())
    })
}

fn save_progress(tx: &Transaction<'_>, id: &str, progress: &Progress) -> Result<()> {
    let mut columns = Columns::default();
    columns.text("id", Some(id));
    columns.message("request_message", Some(&progress.request))?;
    columns.text("owner", Some(&progress.request.owner));
    columns.text("host", Some(&progress.request.host));
    columns.text("pane", progress.pane.as_deref());
    columns.text("terminal", progress.terminal.as_deref());
    columns.flag("attempted", progress.attempted);
    columns.flag("confirmed", progress.confirmed);
    columns.number("finished_ms", progress.finished_ms)?;
    columns.number("closed_ms", progress.closed_ms)?;
    columns.message("result_message", Some(&progress.result))?;
    columns.text("error", progress.error.as_deref());
    match &progress.preflight_refusal {
        Some(PreflightRefusal::LineBudget { bytes }) => {
            columns.text("preflight_kind", Some("line_budget"));
            columns.number("preflight_bytes", Some(*bytes as u64))?;
        }
        Some(PreflightRefusal::Validity { detail }) => {
            columns.text("preflight_kind", Some("validity"));
            columns.text("preflight_detail", Some(detail));
        }
        None => (),
    }
    columns.message(
        "terminal_receipt_message",
        progress.terminal_receipt.as_ref(),
    )?;
    columns.message("stopped_message", progress.stopped.as_ref())?;
    if let Some(release) = &progress.origin_release {
        columns.message("origin_release_certificate", Some(&release.certificate))?;
        columns.number("origin_release_accepted_ms", Some(release.accepted_ms))?;
    }
    columns.upsert(tx, "launch_progress", "id")
}

pub(crate) fn candidates(ledger: &Ledger, pane: Option<&str>) -> Result<Vec<(String, Progress)>> {
    store::read(ledger, |tx| {
        let sql = if pane.is_some() {
            "SELECT * FROM launch_progress WHERE pane = ?1 ORDER BY id"
        } else {
            "SELECT * FROM launch_progress ORDER BY id"
        };
        let mut statement = tx.prepare_cached(sql)?;
        let mut records = if let Some(pane) = pane {
            statement.query([pane])?
        } else {
            statement.query([])?
        };
        let mut found = Vec::new();
        while let Some(row) = records.next()? {
            found.push((required(row, "id")?, progress_row(row)?));
        }
        Ok(found)
    })
}

/// Clock bootstrap reads one consistent snapshot without reopening the database per row.
pub(crate) fn values(ledger: &Ledger) -> Result<Vec<Value>> {
    store::read(ledger, |tx| {
        let mut values = Vec::new();
        let mut statement = tx.prepare_cached("SELECT * FROM launch_progress ORDER BY id")?;
        let mut records = statement.query([])?;
        while let Some(row) = records.next()? {
            values.push(serde_json::to_value(progress_row(row)?)?);
        }
        let mut statement =
            tx.prepare_cached("SELECT result_message FROM launch_receipts ORDER BY id")?;
        let mut records = statement.query([])?;
        while let Some(row) = records.next()? {
            values.push(message(row, "result_message")?);
        }
        for table in ["launch_releases", "launch_reference_releases"] {
            let mut statement = tx.prepare_cached(&format!("SELECT * FROM {table} ORDER BY id"))?;
            let mut records = statement.query([])?;
            while let Some(row) = records.next()? {
                values.push(serde_json::json!({
                    "certificate": message::<Value>(row, "release_certificate")?,
                    "accepted_ms": rows::unsigned(row.get("accepted_ms")?)?,
                }));
            }
        }
        Ok(values)
    })
}

pub(crate) fn receipt(ledger: &Ledger, id: &str) -> Result<Option<Value>> {
    store::read(ledger, |tx| {
        get_message(tx, "launch_receipts", "result_message", id)
    })
}

pub(crate) fn receipt_as<T: DeserializeOwned>(ledger: &Ledger, id: &str) -> Result<Option<T>> {
    receipt(ledger, id)?
        .map(|value| Ok(serde_json::from_value(value)?))
        .transpose()
}

pub(crate) fn put_receipt(ledger: &Ledger, id: &str, receipt: &impl Serialize) -> Result<()> {
    crate::jobs::validate_id(id)?;
    let value = serde_json::to_value(receipt)?;
    let _: crate::launch::Receipt = serde_json::from_value(value.clone())?;
    if !value.is_object() {
        return Err(crate::error::Error::Invalid(
            "launch receipt must be an object".into(),
        ));
    }
    store::write(ledger, |tx| {
        let mut columns = Columns::default();
        columns.text("id", Some(id));
        columns.text("failed", value["failed"].as_str());
        columns.text("pane", value["pane"].as_str());
        columns.text("host", value["host"].as_str());
        columns.message("result_message", Some(&value))?;
        columns.upsert(tx, "launch_receipts", "id")
    })
}

fn get_message<T: DeserializeOwned>(
    tx: &Transaction<'_>,
    table: &str,
    column: &str,
    id: &str,
) -> Result<Option<T>> {
    let mut statement =
        tx.prepare_cached(&format!("SELECT {column} FROM {table} WHERE id = ?1"))?;
    let mut records = statement.query([id])?;
    records.next()?.map(|row| message(row, column)).transpose()
}

macro_rules! release_accessors {
    ($get:ident, $put:ident, $table:literal, $ty:ident) => {
        pub(crate) fn $get(ledger: &Ledger, id: &str) -> Result<Option<$ty>> {
            store::read(ledger, |tx| {
                let mut statement =
                    tx.prepare_cached(concat!("SELECT * FROM ", $table, " WHERE id = ?1"))?;
                let mut records = statement.query([id])?;
                records
                    .next()?
                    .map(|row| -> Result<_> {
                        Ok($ty {
                            certificate: message(row, "release_certificate")?,
                            accepted_ms: rows::unsigned(row.get("accepted_ms")?)?,
                        })
                    })
                    .transpose()
            })
        }
        pub(crate) fn $put(ledger: &Ledger, id: &str, release: &$ty) -> Result<()> {
            crate::jobs::validate_id(id)?;
            store::write(ledger, |tx| {
                let mut columns = Columns::default();
                columns.text("id", Some(id));
                columns.number("accepted_ms", Some(release.accepted_ms))?;
                columns.message("release_certificate", Some(&release.certificate))?;
                columns.upsert(tx, $table, "id")
            })
        }
    };
}
release_accessors!(release, put_release, "launch_releases", OrdinaryReleaseAck);
release_accessors!(
    reference_release,
    put_reference_release,
    "launch_reference_releases",
    ReferenceReleaseAck
);

pub(crate) fn reference_state(
    ledger: &Ledger,
    id: &str,
) -> Result<(Option<ReferenceReleaseAck>, Option<Progress>)> {
    store::read(ledger, |tx| {
        let mut statement =
            tx.prepare_cached("SELECT * FROM launch_reference_releases WHERE id = ?1")?;
        let mut records = statement.query([id])?;
        let release = records
            .next()?
            .map(|row| -> Result<_> {
                Ok(ReferenceReleaseAck {
                    certificate: message(row, "release_certificate")?,
                    accepted_ms: rows::unsigned(row.get("accepted_ms")?)?,
                })
            })
            .transpose()?;
        Ok((release, load_progress(tx, id)?))
    })
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub(crate) enum Record {
    Receipt,
    Progress,
    Release,
    ReferenceRelease,
    Seed,
    Continuations,
}

impl Record {
    fn table(self) -> &'static str {
        match self {
            Self::Receipt => "launch_receipts",
            Self::Progress => "launch_progress",
            Self::Release => "launch_releases",
            Self::ReferenceRelease => "launch_reference_releases",
            Self::Seed => "launch_seeds",
            Self::Continuations => "launch_continuations",
        }
    }
}

pub(crate) fn remove(ledger: &Ledger, record: Record, id: &str) -> Result<()> {
    store::write(ledger, |tx| {
        tx.execute(
            &format!("DELETE FROM {} WHERE id = ?1", record.table()),
            [id],
        )?;
        Ok(())
    })
}

pub(crate) fn remove_family(ledger: &Ledger, id: &str) -> Result<()> {
    store::write(ledger, |tx| {
        for record in [
            Record::Receipt,
            Record::Progress,
            Record::Seed,
            Record::Continuations,
        ] {
            tx.execute(
                &format!("DELETE FROM {} WHERE id = ?1", record.table()),
                [id],
            )?;
        }
        Ok(())
    })
}

pub(crate) fn exists(ledger: &Ledger, id: &str) -> Result<bool> {
    store::read(ledger, |tx| {
        Ok(tx.prepare_cached("SELECT 1 FROM launch_receipts WHERE id = ?1 UNION ALL SELECT 1 FROM launch_progress WHERE id = ?1 UNION ALL SELECT 1 FROM launch_seeds WHERE id = ?1 UNION ALL SELECT 1 FROM launch_releases WHERE id = ?1 UNION ALL SELECT 1 FROM launch_reference_releases WHERE id = ?1 UNION ALL SELECT 1 FROM launch_continuations WHERE id = ?1 LIMIT 1")?.exists([id])?)
    })
}

pub(crate) fn release_ids(ledger: &Ledger) -> Result<Vec<String>> {
    store::read(ledger, |tx| {
        let mut statement = tx.prepare_cached("SELECT id FROM launch_releases ORDER BY id")?;
        Ok(statement
            .query_map([], |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()?)
    })
}

pub(crate) fn progress_entries(ledger: &Ledger) -> Result<Vec<(String, Option<Value>)>> {
    candidates(ledger, None)?
        .into_iter()
        .map(|(id, progress)| Ok((id, Some(serde_json::to_value(progress)?))))
        .collect()
}

pub(crate) fn ids(ledger: &Ledger) -> Result<Vec<String>> {
    store::read(ledger, |tx| {
        let mut statement = tx.prepare_cached("SELECT id FROM launch_receipts UNION SELECT id FROM launch_progress UNION SELECT id FROM launch_releases UNION SELECT id FROM launch_reference_releases UNION SELECT id FROM launch_seeds UNION SELECT id FROM launch_continuations ORDER BY id")?;
        Ok(statement
            .query_map([], |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()?)
    })
}

pub(crate) fn value(ledger: &Ledger, record: Record, id: &str) -> Result<Value> {
    Ok(match record {
        Record::Receipt => serde_json::to_value(receipt(ledger, id)?)?,
        Record::Progress => serde_json::to_value(progress(ledger, id)?)?,
        Record::Release => serde_json::to_value(release(ledger, id)?)?,
        Record::ReferenceRelease => serde_json::to_value(reference_release(ledger, id)?)?,
        Record::Seed => serde_json::to_value(seed(ledger, id)?)?,
        Record::Continuations => serde_json::to_value(continuations(ledger, id)?)?,
    })
}

pub(crate) fn put_value(ledger: &Ledger, record: Record, id: &str, value: Value) -> Result<()> {
    match record {
        Record::Receipt => put_receipt(ledger, id, &value),
        Record::Progress => put_progress(ledger, id, &serde_json::from_value(value)?),
        Record::Release => put_release(ledger, id, &serde_json::from_value(value)?),
        Record::ReferenceRelease => {
            put_reference_release(ledger, id, &serde_json::from_value(value)?)
        }
        Record::Seed => put_seed(ledger, id, &serde_json::from_value(value)?),
        Record::Continuations => put_continuations(ledger, id, &serde_json::from_value(value)?),
    }
}

pub(crate) fn continuations(
    ledger: &Ledger,
    id: &str,
) -> Result<std::collections::BTreeMap<String, crate::continuation::Receipt>> {
    store::read(ledger, |tx| {
        let mut statement = tx.prepare_cached(
            "SELECT * FROM launch_continuation_receipts WHERE launch = ?1 ORDER BY position",
        )?;
        let mut records = statement.query([id])?;
        let mut found = std::collections::BTreeMap::new();
        while let Some(row) = records.next()? {
            found.insert(
                required(row, "series")?,
                crate::continuation::Receipt {
                    request: message(row, "request_message")?,
                    mark: message(row, "mark_message")?,
                    sent: message(row, "sent_message")?,
                    phase: rows::variant(required(row, "phase")?)?,
                },
            );
        }
        Ok(found)
    })
}

pub(crate) fn put_continuations(
    ledger: &Ledger,
    id: &str,
    receipts: &std::collections::BTreeMap<String, crate::continuation::Receipt>,
) -> Result<()> {
    crate::jobs::validate_id(id)?;
    store::write(ledger, |tx| {
        tx.execute(
            "INSERT INTO launch_continuations (id) VALUES (?1) ON CONFLICT (id) DO NOTHING",
            [id],
        )?;
        tx.execute(
            "DELETE FROM launch_continuation_receipts WHERE launch = ?1",
            [id],
        )?;
        let mut insert = tx.prepare_cached("INSERT INTO launch_continuation_receipts (launch, position, series, phase, request_message, mark_message, sent_message) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)")?;
        for (position, (series, receipt)) in (0_i64..).zip(receipts) {
            insert.execute(rusqlite::params![
                id,
                position,
                series,
                rows::name(&receipt.phase)?,
                rows::message(&receipt.request)?,
                rows::message(&receipt.mark)?,
                rows::message(&receipt.sent)?
            ])?;
        }
        Ok(())
    })
}

pub(crate) const SCHEMA: &str = concat!(
    "CREATE TABLE launch_seeds (
        id TEXT PRIMARY KEY CHECK (length(id) > 0),
        version INTEGER NOT NULL CHECK (version >= 0),
        executor TEXT NOT NULL,
        owner_id TEXT NOT NULL,
        request_id TEXT NOT NULL,
        session_id TEXT NOT NULL,
        unit_id TEXT NOT NULL,
        revision INTEGER NOT NULL CHECK (revision >= 0),
        contract_entry_id TEXT NOT NULL,
        binding_executor TEXT NOT NULL,
        binding_owner_id TEXT NOT NULL,
        binding_request_id TEXT NOT NULL,
        export_digest TEXT NOT NULL,
        export_message BLOB NOT NULL
    ) STRICT;
    ",
    "CREATE INDEX launch_seeds_execution ON launch_seeds (executor, owner_id, request_id);
    CREATE TABLE launch_progress (
        id TEXT PRIMARY KEY CHECK (length(id) > 0),
        request_message BLOB NOT NULL,
        owner TEXT NOT NULL,
        host TEXT NOT NULL,
        pane TEXT,
        terminal TEXT,
        attempted INTEGER NOT NULL CHECK (attempted IN (0, 1)),
        confirmed INTEGER NOT NULL CHECK (confirmed IN (0, 1)),
        finished_ms INTEGER CHECK (finished_ms >= 0),
        closed_ms INTEGER CHECK (closed_ms >= 0),
        result_message BLOB NOT NULL,
        error TEXT,
        preflight_kind TEXT CHECK (preflight_kind IN ('line_budget', 'validity')),
        preflight_bytes INTEGER CHECK (preflight_bytes >= 0),
        preflight_detail TEXT,
        terminal_receipt_message BLOB,
        stopped_message BLOB,
        origin_release_certificate BLOB,
        origin_release_accepted_ms INTEGER CHECK (origin_release_accepted_ms >= 0),
        CHECK ((preflight_kind = 'line_budget') = (preflight_bytes IS NOT NULL)),
        CHECK ((preflight_kind = 'validity') = (preflight_detail IS NOT NULL)),
        CHECK ((origin_release_certificate IS NULL) = (origin_release_accepted_ms IS NULL))
    ) STRICT;
    CREATE INDEX launch_progress_pane ON launch_progress (pane);
    CREATE INDEX launch_progress_owner ON launch_progress (owner);
    CREATE TABLE launch_receipts (
        id TEXT PRIMARY KEY CHECK (length(id) > 0),
        failed TEXT,
        pane TEXT,
        host TEXT,
        result_message BLOB NOT NULL
    ) STRICT;
    CREATE INDEX launch_receipts_pane ON launch_receipts (pane);
    CREATE TABLE launch_releases (
        id TEXT PRIMARY KEY CHECK (length(id) > 0),
        accepted_ms INTEGER NOT NULL CHECK (accepted_ms >= 0),
        release_certificate BLOB NOT NULL
    ) STRICT;
    CREATE TABLE launch_reference_releases (
        id TEXT PRIMARY KEY CHECK (length(id) > 0),
        accepted_ms INTEGER NOT NULL CHECK (accepted_ms >= 0),
        release_certificate BLOB NOT NULL
    ) STRICT;
    CREATE TABLE launch_continuations (
        id TEXT PRIMARY KEY CHECK (length(id) > 0)
    ) STRICT;
    CREATE TABLE launch_continuation_receipts (
        launch TEXT NOT NULL REFERENCES launch_continuations (id) ON DELETE CASCADE,
        position INTEGER NOT NULL CHECK (position >= 0),
        series TEXT NOT NULL,
        phase TEXT NOT NULL CHECK (phase IN ('prepared', 'accepted')),
        request_message BLOB NOT NULL,
        mark_message BLOB NOT NULL,
        sent_message BLOB NOT NULL,
        PRIMARY KEY (launch, position),
        UNIQUE (launch, series)
    ) STRICT;"
);

pub(crate) fn put_seed(
    ledger: &Ledger,
    id: &str,
    seed: &crate::unit_transport::Seed,
) -> Result<()> {
    crate::jobs::validate_id(id)?;
    store::write(ledger, |tx| {
        let mut columns = Columns::default();
        columns.text("id", Some(id));
        columns.number("version", Some(u64::from(seed.version)))?;
        columns.text("executor", Some(&seed.execution.executor));
        columns.text("owner_id", Some(&seed.execution.owner_id));
        columns.text("request_id", Some(&seed.execution.request_id));
        columns.text("session_id", Some(&seed.binding.contract.session_id));
        columns.text("unit_id", Some(&seed.binding.contract.unit_id));
        columns.number("revision", Some(u64::from(seed.binding.contract.revision)))?;
        columns.text(
            "contract_entry_id",
            Some(&seed.binding.contract.contract_entry_id),
        );
        columns.text("binding_executor", Some(&seed.binding.execution.executor));
        columns.text("binding_owner_id", Some(&seed.binding.execution.owner_id));
        columns.text(
            "binding_request_id",
            Some(&seed.binding.execution.request_id),
        );
        columns.text("export_digest", Some(&seed.binding.export_digest));
        columns.message("export_message", Some(&seed.exported))?;
        columns.upsert(tx, "launch_seeds", "id")
    })
}

pub(crate) fn seed(ledger: &Ledger, id: &str) -> Result<Option<crate::unit_transport::Seed>> {
    store::read(ledger, |tx| {
        let mut statement = tx.prepare_cached("SELECT * FROM launch_seeds WHERE id = ?1")?;
        let mut records = statement.query([id])?;
        let Some(row) = records.next()? else {
            return Ok(None);
        };
        let execution = |executor, owner_id, request_id| -> Result<_> {
            Ok(crate::unit_transport::ExecutionRef {
                executor: rows::required(row, executor)?,
                owner_id: rows::required(row, owner_id)?,
                request_id: rows::required(row, request_id)?,
            })
        };
        Ok(Some(crate::unit_transport::Seed {
            version: row.get("version")?,
            execution: execution("executor", "owner_id", "request_id")?,
            binding: crate::unit_transport::Binding {
                contract: crate::unit_transport::UnitRef {
                    session_id: rows::required(row, "session_id")?,
                    unit_id: rows::required(row, "unit_id")?,
                    revision: row.get("revision")?,
                    contract_entry_id: rows::required(row, "contract_entry_id")?,
                },
                execution: execution("binding_executor", "binding_owner_id", "binding_request_id")?,
                export_digest: rows::required(row, "export_digest")?,
            },
            exported: rows::decoded(row, "export_message")?.ok_or_else(|| {
                crate::error::Error::Invalid("stored seed export is missing".into())
            })?,
        }))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ledger() -> Ledger {
        Ledger::new(std::env::temp_dir().join(crate::ledger::new_id()))
    }

    fn seed_value() -> Value {
        let fixtures: Value = serde_json::from_str(include_str!(
            "../crates/todo-core/tests/fixtures/unit-exports.json"
        ))
        .unwrap();
        let mut export: Value =
            serde_json::from_str(fixtures[0]["canonical"].as_str().unwrap()).unwrap();
        export["digest"] = fixtures[0]["digest"].clone();
        let execution = json!({"executor":"dogsled","ownerId":"owner","requestId":"test"});
        json!({"version":1,"execution":execution,"binding":{"contract":export["contract"]["ref"],"execution":execution,"exportDigest":export["digest"]},"exported":export})
    }

    fn launch() -> Value {
        json!({"id":"test","validity":{"version":1,"created_ms":3},"preparation_protocol":1,
            "action":{"type":"agent","kind":"pi","name":"worker","message":"go","profile":"worker"},
            "unit":seed_value(),"host":"local","cwd":"/checkout","pane":"pane","terminal":"terminal",
            "workspace":"workspace","workspace_name":"workspace-name","owner":"owner","owner_host":"origin"})
    }

    fn reference() -> Value {
        json!({"version":1,"origin_host":"origin","execution":seed_value()["execution"],
            "binding":seed_value()["binding"],"launch":launch(),
            "terminal_evidence":{"kind":"stopped","launch":launch(),"reason":"stopped"},
            "custody_revision":7,"released_references":["execution","recovery","task_binding","custody"]})
    }

    fn round_trip(record: Record, value: Value) {
        let ledger = ledger();
        let expected = match record {
            Record::Seed => serde_json::to_value(
                serde_json::from_value::<crate::unit_transport::Seed>(value.clone()).unwrap(),
            )
            .unwrap(),
            Record::Release => serde_json::to_value(
                serde_json::from_value::<OrdinaryReleaseAck>(value.clone()).unwrap(),
            )
            .unwrap(),
            Record::ReferenceRelease => serde_json::to_value(
                serde_json::from_value::<ReferenceReleaseAck>(value.clone()).unwrap(),
            )
            .unwrap(),
            _ => value.clone(),
        };
        put_value(&ledger, record, "test", value).unwrap();
        // Compare the Rust type's canonical serialization (serde may omit default fields).
        let first = super::value(&ledger, record, "test").unwrap();
        assert_eq!(first, expected);
        put_value(&ledger, record, "test", first.clone()).unwrap();
        assert_eq!(super::value(&ledger, record, "test").unwrap(), first);
        assert!(put_value(&ledger, record, "bad", Value::Null).is_err());
        assert!(!exists(&ledger, "bad").unwrap());
        remove(&ledger, record, "test").unwrap();
        assert!(!exists(&ledger, "test").unwrap());
        std::fs::remove_dir_all(ledger.dir).unwrap();
    }

    #[test]
    fn seed_round_trip() {
        round_trip(Record::Seed, seed_value());
    }

    #[test]
    fn receipt_round_trip() {
        round_trip(
            Record::Receipt,
            json!({"id":"test","host":"local","pane":"pane","name":"worker",
            "launch_confirmed_ms":4,"launch":launch(),"request":{"cmd":["echo","go"]},"checkout":{"repo":"/repo","path":"/checkout","branch":"branch","head":"head"}}),
        );
    }

    #[test]
    fn progress_round_trip_and_update_clears_optional_evidence() {
        let ledger = ledger();
        let full = json!({"request":launch(),"finished_ms":8,"closed_ms":9,"pane":"pane","terminal":"terminal",
            "attempted":true,"confirmed":true,"result":{"pane":"pane"},"error":"error",
            "preflight_refusal":{"kind":"line_budget","bytes":1001},
            "terminal_receipt":{"ack":{"execution":seed_value()["execution"],"unitRef":seed_value()["binding"]["contract"],"digest":"digest","sealEntryId":"seal","sessionLeafId":"leaf"},"sessionId":"test","replyEntryId":"reply","text":"FINAL"},
            "stopped":{"kind":"stopped","launch":launch(),"reason":"stopped"},
            "origin_release":{"certificate":reference(),"accepted_ms":10}});
        let expected =
            serde_json::to_value(serde_json::from_value::<Progress>(full.clone()).unwrap())
                .unwrap();
        put_value(&ledger, Record::Progress, "test", full.clone()).unwrap();
        assert_eq!(value(&ledger, Record::Progress, "test").unwrap(), expected);
        let mut cleared = full;
        for key in [
            "finished_ms",
            "closed_ms",
            "terminal_receipt",
            "stopped",
            "origin_release",
            "preflight_refusal",
        ] {
            cleared.as_object_mut().unwrap().remove(key);
        }
        put_value(&ledger, Record::Progress, "test", cleared.clone()).unwrap();
        assert_eq!(
            value(&ledger, Record::Progress, "test").unwrap(),
            serde_json::to_value(serde_json::from_value::<Progress>(cleared).unwrap()).unwrap()
        );
        assert!(put_value(&ledger, Record::Progress, "bad", json!({})).is_err());
        assert!(!exists(&ledger, "bad").unwrap());
        std::fs::remove_dir_all(ledger.dir).unwrap();
    }

    #[test]
    fn release_round_trip() {
        let mut ordinary = launch();
        ordinary.as_object_mut().unwrap().remove("unit");
        round_trip(
            Record::Release,
            json!({"certificate":{"version":2,"origin_host":"origin",
            "execution":seed_value()["execution"],"launch":ordinary,"binding":{"kind":"none"},
            "terminal_evidence":{"kind":"agent","pane":"pane","terminal":"terminal","closed_ms":9},
            "custody_revision":7,"released_references":["execution","recovery","custody"]},"accepted_ms":10}),
        );
    }

    #[test]
    fn reference_release_round_trip() {
        round_trip(
            Record::ReferenceRelease,
            json!({"certificate":reference(),"accepted_ms":10}),
        );
    }

    #[test]
    fn continuation_parent_and_ordered_receipts_round_trip_and_clear_children() {
        let ledger = ledger();
        let owner =
            json!({"name":"owner","pane":"owner-pane","terminal":"owner-terminal","host":"origin"});
        let receipt = json!({"request":{"request":"test","series":"series","owner":owner,"session":"session",
            "worker":{"pane":"pane","terminal":"terminal","agent_session":"session","agent":"pi","name":"worker"},"kind":"message","text_sha256":"digest"},
            "mark":{"pane":"pane","item":"test","session":"session","name":"worker","after":1,"sealed":2,"marked_ms":3,"detached":false,"native_done":true},
            "sent":{"id":"sent","host":"local","host_label":"local","terminal":"terminal","pane":"pane","name":"worker","harness":"pi","from":owner,"kind":"message","created_ms":4,"state":"pending","check":{"attempts":1,"next_ms":5}},"phase":"prepared"});
        let records = json!({"series":receipt});
        put_value(&ledger, Record::Continuations, "test", records.clone()).unwrap();
        let expected = serde_json::to_value(
            serde_json::from_value::<
                std::collections::BTreeMap<String, crate::continuation::Receipt>,
            >(records)
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            value(&ledger, Record::Continuations, "test").unwrap(),
            expected
        );
        put_value(&ledger, Record::Continuations, "test", json!({})).unwrap();
        assert!(continuations(&ledger, "test").unwrap().is_empty());
        assert!(put_value(&ledger, Record::Continuations, "bad", json!({"bad":null})).is_err());
        assert!(!exists(&ledger, "bad").unwrap());
        std::fs::remove_dir_all(ledger.dir).unwrap();
    }
}
