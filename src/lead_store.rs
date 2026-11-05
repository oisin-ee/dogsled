//! Typed, host-local lead records. Launch intent is immutable; outcomes are updated
//! transactionally. Leads have no foreign key to child items, jobs, or their retention.

use crate::error::{Error, Result};
use crate::leads::Record;
use crate::ledger::Ledger;
use crate::rows::{self, Columns, number, required, text};
use crate::store;
use rusqlite::{Row, Transaction};

pub(crate) const SCHEMA: &str = "
CREATE TABLE leads (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    repository_path TEXT NOT NULL,
    repository_origin TEXT NOT NULL,
    repository_relative TEXT NOT NULL,
    repository_working_root TEXT NOT NULL,
    prompt TEXT NOT NULL,
    validity_version INTEGER NOT NULL CHECK (validity_version >= 0),
    validity_created_ms INTEGER NOT NULL CHECK (validity_created_ms >= 0),
    created_ms INTEGER NOT NULL CHECK (created_ms >= 0),
    phase TEXT NOT NULL CHECK (phase IN (
        'recorded', 'allocating', 'allocated', 'starting', 'started', 'prompting', 'ready'
    )),
    pane TEXT,
    terminal TEXT,
    workspace TEXT,
    updated_ms INTEGER NOT NULL CHECK (updated_ms >= 0),
    error TEXT,
    close_requested_ms INTEGER CHECK (close_requested_ms >= 0),
    closed_ms INTEGER CHECK (closed_ms >= 0)
) STRICT;
CREATE INDEX leads_pane ON leads (pane);
";

pub(crate) fn get(ledger: &Ledger, id: &str) -> Result<Option<Record>> {
    store::read(ledger, |tx| get_in(tx, id))
}

fn get_in(tx: &Transaction<'_>, id: &str) -> Result<Option<Record>> {
    let mut statement = tx.prepare_cached("SELECT * FROM leads WHERE id = ?1")?;
    let mut records = statement.query([id])?;
    records.next()?.map(load).transpose()
}

pub(crate) fn list(ledger: &Ledger) -> Result<Vec<Record>> {
    store::read(ledger, |tx| {
        let mut statement = tx.prepare_cached("SELECT * FROM leads ORDER BY created_ms, id")?;
        let mut rows = statement.query([])?;
        let mut records = Vec::new();
        while let Some(row) = rows.next()? {
            records.push(load(row)?);
        }
        Ok(records)
    })
}

pub(crate) fn exists_in(tx: &Transaction<'_>, id: &str) -> Result<bool> {
    Ok(tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM leads WHERE id = ?1)",
        [id],
        |row| row.get(0),
    )?)
}

pub(crate) fn insert(ledger: &Ledger, record: &Record) -> Result<()> {
    crate::jobs::validate_id(&record.id)?;
    if record.name != format!("lead-{}", record.id) {
        return Err(Error::Invalid("lead name must be lead-<request-id>".into()));
    }
    store::write(ledger, |tx| {
        if exists_in(tx, &record.id)? {
            return Err(Error::Invalid(format!("lead {} already exists", record.id)));
        }
        save(tx, record)
    })
}

/// Update only an existing outcome. The saved intent cannot be replaced on replay.
pub(crate) fn update(
    ledger: &Ledger,
    id: &str,
    update: impl FnOnce(&mut Record) -> Result<()>,
) -> Result<()> {
    store::write(ledger, |tx| {
        let mut record =
            get_in(tx, id)?.ok_or_else(|| Error::Invalid(format!("lead {id} not found")))?;
        let original = record.clone();
        update(&mut record)?;
        if record.id != original.id
            || record.name != original.name
            || record.repository.path != original.repository.path
            || record.repository.origin != original.repository.origin
            || record.repository.relative != original.repository.relative
            || record.repository.working_root != original.repository.working_root
            || record.prompt != original.prompt
            || record.validity != original.validity
            || record.created_ms != original.created_ms
        {
            return Err(Error::Invalid("lead launch intent changed".into()));
        }
        save(tx, &record)
    })
}

fn load(row: &Row<'_>) -> Result<Record> {
    Ok(Record {
        id: required(row, "id")?,
        name: required(row, "name")?,
        repository: crate::checkout::Repository {
            path: required(row, "repository_path")?,
            origin: required(row, "repository_origin")?,
            relative: required(row, "repository_relative")?,
            working_root: required(row, "repository_working_root")?,
        },
        prompt: required(row, "prompt")?,
        validity: crate::jobs::Validity {
            version: u32::try_from(row.get::<_, i64>("validity_version")?)
                .map_err(|_| Error::Invalid("invalid lead validity version".into()))?,
            created_ms: rows::unsigned(row.get("validity_created_ms")?)?,
        },
        created_ms: rows::unsigned(row.get("created_ms")?)?,
        phase: rows::variant(required(row, "phase")?)?,
        pane: text(row, "pane")?,
        terminal: text(row, "terminal")?,
        workspace: text(row, "workspace")?,
        updated_ms: rows::unsigned(row.get("updated_ms")?)?,
        error: text(row, "error")?,
        close_requested_ms: number(row, "close_requested_ms")?,
        closed_ms: number(row, "closed_ms")?,
    })
}

fn save(tx: &Transaction<'_>, record: &Record) -> Result<()> {
    let mut columns = Columns::default();
    columns.text("id", Some(&record.id));
    columns.text("name", Some(&record.name));
    columns.text("repository_path", Some(&record.repository.path));
    columns.text("repository_origin", Some(&record.repository.origin));
    columns.text("repository_relative", Some(&record.repository.relative));
    columns.text(
        "repository_working_root",
        Some(&record.repository.working_root),
    );
    columns.text("prompt", Some(&record.prompt));
    columns.number("validity_version", Some(u64::from(record.validity.version)))?;
    columns.number("validity_created_ms", Some(record.validity.created_ms))?;
    columns.number("created_ms", Some(record.created_ms))?;
    columns.text("phase", Some(&rows::name(&record.phase)?));
    columns.text("pane", record.pane.as_deref());
    columns.text("terminal", record.terminal.as_deref());
    columns.text("workspace", record.workspace.as_deref());
    columns.number("updated_ms", Some(record.updated_ms))?;
    columns.text("error", record.error.as_deref());
    columns.number("close_requested_ms", record.close_requested_ms)?;
    columns.number("closed_ms", record.closed_ms)?;
    columns.upsert(tx, "leads", "id")
}
