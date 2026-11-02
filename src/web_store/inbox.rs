use std::collections::BTreeMap;

use rusqlite::{Row, Transaction};

use crate::backoff::Backoff;
use crate::error::Result;
use crate::inbox::{Reader, Record, Sender, Sent};
use crate::rows::{self, Columns, number, required, text};

pub(crate) fn terminals(tx: &Transaction<'_>) -> Result<Vec<String>> {
    Ok(tx
        .prepare_cached("SELECT id FROM inbox_terminals ORDER BY id")?
        .query_map([], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?)
}

fn terminal(tx: &Transaction<'_>, id: &str) -> Result<()> {
    crate::inbox::validate_terminal(id)?;
    tx.execute(
        "INSERT INTO inbox_terminals(id) VALUES (?1) ON CONFLICT DO NOTHING",
        [id],
    )?;
    Ok(())
}

pub(crate) fn remove_terminal(tx: &Transaction<'_>, id: &str) -> Result<()> {
    tx.execute("DELETE FROM inbox_terminals WHERE id = ?1", [id])?;
    Ok(())
}

pub(crate) fn reader(tx: &Transaction<'_>, id: &str) -> Result<Option<Reader>> {
    let mut statement = tx.prepare_cached("SELECT * FROM inbox_readers WHERE terminal = ?1")?;
    let mut rows = statement.query([id])?;
    rows.next()?
        .map(|row| {
            Ok(Reader {
                harness: required(row, "harness")?,
                pane: required(row, "pane")?,
                registered_ms: rows::unsigned(row.get("registered_ms")?)?,
            })
        })
        .transpose()
}

pub(crate) fn put_reader(tx: &Transaction<'_>, id: &str, reader: &Reader) -> Result<()> {
    terminal(tx, id)?;
    let mut columns = Columns::default();
    columns.text("terminal", Some(id));
    columns.text("harness", Some(&reader.harness));
    columns.text("pane", Some(&reader.pane));
    columns.number("registered_ms", Some(reader.registered_ms))?;
    columns.upsert(tx, "inbox_readers", "terminal")
}

pub(crate) fn remove_reader(tx: &Transaction<'_>, id: &str) -> Result<()> {
    tx.execute("DELETE FROM inbox_readers WHERE terminal = ?1", [id])?;
    Ok(())
}

fn sender(row: &Row<'_>) -> Result<Sender> {
    Ok(Sender {
        name: text(row, "sender_name")?,
        pane: text(row, "sender_pane")?,
        terminal: text(row, "sender_terminal")?,
        host: required(row, "sender_host")?,
    })
}

fn sender_columns(columns: &mut Columns, from: &Sender) {
    columns.text("sender_name", from.name.as_deref());
    columns.text("sender_pane", from.pane.as_deref());
    columns.text("sender_terminal", from.terminal.as_deref());
    columns.text("sender_host", Some(&from.host));
}

fn backoff(row: &Row<'_>) -> Result<Backoff> {
    Ok(Backoff {
        attempts: row.get("attempts")?,
        next_ms: rows::unsigned(row.get("next_ms")?)?,
    })
}

pub(crate) fn records(tx: &Transaction<'_>, id: &str) -> Result<Vec<Record>> {
    let mut statement =
        tx.prepare_cached("SELECT * FROM inbox_records WHERE terminal = ?1 ORDER BY position")?;
    let mut rows = statement.query([id])?;
    let mut records = Vec::new();
    while let Some(row) = rows.next()? {
        records.push(Record {
            id: required(row, "record_id")?,
            terminal: required(row, "terminal")?,
            pane: required(row, "pane")?,
            from: sender(row)?,
            kind: required(row, "kind")?,
            text: required(row, "text")?,
            created_ms: rows::unsigned(row.get("created_ms")?)?,
            state: rows::variant(required(row, "state")?)?,
            lease: backoff(row)?,
            reason: text(row, "reason")?,
            done_ms: number(row, "done_ms")?,
        });
    }
    Ok(records)
}

pub(crate) fn all_records(tx: &Transaction<'_>) -> Result<Vec<Record>> {
    let mut all = Vec::new();
    for terminal in terminals(tx)? {
        all.extend(records(tx, &terminal)?);
    }
    Ok(all)
}

pub(crate) fn put_records(tx: &Transaction<'_>, id: &str, records: &[Record]) -> Result<()> {
    terminal(tx, id)?;
    tx.execute("DELETE FROM inbox_records WHERE terminal = ?1", [id])?;
    for (position, record) in records.iter().enumerate() {
        if record.terminal != id {
            return Err(crate::error::Error::Invalid(
                "inbox terminal mismatch".into(),
            ));
        }
        let mut columns = Columns::default();
        columns.text("id", Some(&format!("{id}:{}", record.id)));
        columns.text("record_id", Some(&record.id));
        columns.text("terminal", Some(id));
        columns.number("position", Some(position as u64))?;
        columns.text("pane", Some(&record.pane));
        sender_columns(&mut columns, &record.from);
        columns.text("kind", Some(&record.kind));
        columns.text("text", Some(&record.text));
        columns.number("created_ms", Some(record.created_ms))?;
        columns.text("state", Some(&rows::name(&record.state)?));
        columns.number("attempts", Some(u64::from(record.lease.attempts)))?;
        columns.number("next_ms", Some(record.lease.next_ms))?;
        columns.text("reason", record.reason.as_deref());
        columns.number("done_ms", record.done_ms)?;
        columns.upsert(tx, "inbox_records", "id")?;
    }
    Ok(())
}

pub(crate) fn sent(tx: &Transaction<'_>) -> Result<Vec<Sent>> {
    let mut statement = tx.prepare_cached("SELECT * FROM inbox_sent ORDER BY seq")?;
    let mut rows = statement.query([])?;
    let mut records = Vec::new();
    while let Some(row) = rows.next()? {
        records.push(Sent {
            id: required(row, "id")?,
            host: required(row, "host")?,
            host_label: required(row, "host_label")?,
            terminal: required(row, "terminal")?,
            pane: required(row, "pane")?,
            name: text(row, "name")?,
            harness: text(row, "harness")?,
            from: sender(row)?,
            kind: required(row, "kind")?,
            created_ms: rows::unsigned(row.get("created_ms")?)?,
            state: rows::variant(required(row, "state")?)?,
            reason: text(row, "reason")?,
            check: backoff(row)?,
        });
    }
    Ok(records)
}

pub(crate) fn put_sent(tx: &Transaction<'_>, records: &[Sent]) -> Result<()> {
    let old = sent(tx)?;
    for record in records {
        if old.iter().any(|s| s == record) {
            continue;
        }
        crate::inbox::validate_terminal(&record.terminal)?;
        let mut columns = Columns::default();
        columns.text("id", Some(&record.id));
        columns.text("host", Some(&record.host));
        columns.text("host_label", Some(&record.host_label));
        columns.text("terminal", Some(&record.terminal));
        columns.text("pane", Some(&record.pane));
        columns.text("name", record.name.as_deref());
        columns.text("harness", record.harness.as_deref());
        sender_columns(&mut columns, &record.from);
        columns.text("kind", Some(&record.kind));
        columns.number("created_ms", Some(record.created_ms))?;
        columns.text("state", Some(&rows::name(&record.state)?));
        columns.text("reason", record.reason.as_deref());
        columns.number("attempts", Some(u64::from(record.check.attempts)))?;
        columns.number("next_ms", Some(record.check.next_ms))?;
        columns.upsert(tx, "inbox_sent", "id")?;
    }
    for record in old {
        if !records.iter().any(|s| s.id == record.id) {
            tx.execute("DELETE FROM inbox_sent WHERE id = ?1", [&record.id])?;
        }
    }
    Ok(())
}

pub(crate) fn failures(tx: &Transaction<'_>) -> Result<BTreeMap<String, String>> {
    Ok(tx
        .prepare_cached("SELECT id, reason FROM notification_failures ORDER BY id")?
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?)
}

pub(crate) fn put_failure(tx: &Transaction<'_>, id: &str, reason: &str) -> Result<()> {
    let mut columns = Columns::default();
    columns.text("id", Some(id));
    columns.text("reason", Some(reason));
    columns.upsert(tx, "notification_failures", "id")
}
