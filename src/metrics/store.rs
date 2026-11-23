//! Derived data, isolated from state.db and its observation wake notifications.
use rusqlite::{Connection, Transaction, TransactionBehavior};

use crate::error::{Error, Result};
use crate::ledger::Ledger;

// Rebuild the derived store so existing transcripts are replayed with size accounting.
const VERSION: i64 = 2;
const SCHEMA: &str = "
CREATE TABLE transcripts (
 path TEXT PRIMARY KEY NOT NULL,
 harness TEXT NOT NULL CHECK(harness IN ('pi','claude','codex')),
 file_id TEXT NOT NULL, offset INTEGER NOT NULL CHECK(offset >= 0),
 session TEXT, cwd TEXT, repository TEXT, pane TEXT, item TEXT, lead TEXT, model TEXT,
 indexed_ms INTEGER NOT NULL
) STRICT;
CREATE TABLE tool_calls (
 path TEXT NOT NULL REFERENCES transcripts(path) ON DELETE CASCADE,
 call_id TEXT NOT NULL, tool TEXT NOT NULL, model TEXT,
 started_ms INTEGER NOT NULL, ended_ms INTEGER, error INTEGER CHECK(error IN (0,1)),
 result_chars INTEGER CHECK(result_chars >= 0),
 PRIMARY KEY(path,call_id)
) STRICT;
CREATE INDEX tool_calls_started ON tool_calls(started_ms);
CREATE INDEX tool_calls_ended ON tool_calls(ended_ms) WHERE result_chars IS NOT NULL;
CREATE TABLE injections (
 path TEXT NOT NULL REFERENCES transcripts(path) ON DELETE CASCADE,
 kind TEXT NOT NULL, at_ms INTEGER NOT NULL CHECK(at_ms >= 0),
 chars INTEGER NOT NULL CHECK(chars >= 0)
) STRICT;
CREATE INDEX injections_at ON injections(at_ms);
CREATE TABLE index_state (
 id INTEGER PRIMARY KEY CHECK(id=1), indexed_ms INTEGER NOT NULL CHECK(indexed_ms>=0)
) STRICT;
CREATE TABLE input_entries (
 path TEXT NOT NULL REFERENCES transcripts(path) ON DELETE CASCADE,
 at_ms INTEGER NOT NULL CHECK(at_ms >= 0), PRIMARY KEY(path,at_ms)
) STRICT, WITHOUT ROWID;
CREATE TABLE owner_waits (
 id TEXT PRIMARY KEY NOT NULL, kind TEXT NOT NULL CHECK(kind IN ('question','plan_review','ask','blocked')),
 pane TEXT, repository TEXT, lead TEXT, started_ms INTEGER NOT NULL, ended_ms INTEGER
) STRICT;
CREATE TABLE runs (
 item TEXT PRIMARY KEY NOT NULL, host TEXT NOT NULL, harness TEXT, profile TEXT, model TEXT,
 role TEXT, lead TEXT, repository TEXT, queued_ms INTEGER, created_ms INTEGER NOT NULL,
 confirmed_ms INTEGER, done_ms INTEGER, closed_ms INTEGER,
 outcome TEXT NOT NULL CHECK(outcome IN ('running','done','ask','blocked','closed','dropped'))
) STRICT;
";

pub(super) fn unsigned(value: i64) -> rusqlite::Result<u64> {
    u64::try_from(value).map_err(|_| rusqlite::Error::IntegralValueOutOfRange(0, value))
}

fn version(connection: &Connection) -> Result<i64> {
    let version = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if version > VERSION {
        return Err(Error::Invalid(format!(
            "metrics.db schema version {version} is newer than supported version {VERSION}"
        )));
    }
    Ok(version)
}

pub(super) fn open(ledger: &Ledger) -> Result<Connection> {
    let directory = std::path::absolute(&ledger.dir)?;
    if directory.join("metrics.db-wal").as_os_str().len() >= 512 {
        return Err(Error::Invalid(
            "metrics.db-wal path exceeds SQLite's limit".into(),
        ));
    }
    std::fs::create_dir_all(&directory)?;
    let mut connection = Connection::open(directory.join("metrics.db"))?;
    connection.busy_timeout(ledger.budget.remaining("metrics.db")?)?;
    let current = version(&connection)?;
    connection.execute_batch("PRAGMA synchronous=NORMAL;")?;
    if current < VERSION {
        ledger.with_lock("metrics-db", || {
            let current = version(&connection)?;
            connection.query_row("PRAGMA journal_mode=WAL", [], |_| Ok(()))?;
            if current < VERSION {
                let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
                let tables = tx.prepare("SELECT name FROM sqlite_schema WHERE type='table' AND name NOT LIKE 'sqlite_%'")?
                    .query_map([], |row| row.get::<_, String>(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                for table in tables {
                    tx.execute_batch(&format!("DROP TABLE \"{}\"", table.replace('"', "\"\"")))?;
                }
                tx.execute_batch(SCHEMA)?;
                tx.pragma_update(None, "user_version", VERSION)?;
                tx.commit()?;
            }
            Ok(())
        })?;
    } else {
        connection.query_row("PRAGMA journal_mode=WAL", [], |_| Ok(()))?;
    }
    connection.execute_batch("PRAGMA foreign_keys=ON;")?;
    Ok(connection)
}

pub(super) fn write<T>(
    connection: &mut Connection,
    f: impl FnOnce(&Transaction<'_>) -> Result<T>,
) -> Result<T> {
    let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let result = f(&tx)?;
    tx.commit()?;
    Ok(result)
}

pub(super) fn retain(connection: &mut Connection, now: u64) -> Result<()> {
    let before = crate::rows::integer(now.saturating_sub(crate::jobs::RECORD_KEEP_MS))?;
    write(connection, |tx| {
        for (table, column) in [
            ("tool_calls", "COALESCE(ended_ms,started_ms)"),
            ("input_entries", "at_ms"),
            ("injections", "at_ms"),
            ("owner_waits", "started_ms"),
            ("runs", "created_ms"),
        ] {
            tx.execute(
                &format!("DELETE FROM {table} WHERE {column} < ?1"),
                [before],
            )?;
        }
        // Offsets remain for live files; stale transcripts and their dependent rows expire.
        tx.execute("DELETE FROM transcripts WHERE indexed_ms < ?1", [before])?;
        Ok(())
    })
}
