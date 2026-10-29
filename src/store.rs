//! Per-host transactional storage. Record shapes belong to the callers.

use std::collections::HashMap;

use rusqlite::{Connection, Transaction, TransactionBehavior};
use serde::{Serialize, de::DeserializeOwned};

use crate::error::{Error, Result};
use crate::ledger::Ledger;

const SCHEMA: &str = "
CREATE TABLE items (
    seq INTEGER PRIMARY KEY,
    record TEXT NOT NULL CHECK (json_valid(record)),
    id TEXT GENERATED ALWAYS AS (json_extract(record, '$.id')) STORED NOT NULL UNIQUE,
    host TEXT GENERATED ALWAYS AS (json_extract(record, '$.host')) STORED NOT NULL,
    pane TEXT GENERATED ALWAYS AS (json_extract(record, '$.pane')) STORED NOT NULL,
    closed_ms INTEGER GENERATED ALWAYS AS (json_extract(record, '$.closed_ms')) STORED
);
CREATE INDEX items_open_pane ON items (host, pane) WHERE closed_ms IS NULL;
CREATE TABLE tickets (
    seq INTEGER PRIMARY KEY,
    record TEXT NOT NULL CHECK (json_valid(record)),
    id TEXT GENERATED ALWAYS AS (json_extract(record, '$.id')) STORED NOT NULL UNIQUE,
    request_id TEXT GENERATED ALWAYS AS (json_extract(record, '$.request.id')) STORED NOT NULL UNIQUE,
    state TEXT GENERATED ALWAYS AS (json_extract(record, '$.state')) STORED NOT NULL
);
PRAGMA user_version = 1;
";

#[derive(Debug, Clone, Copy)]
pub(crate) enum Table {
    Items,
    Tickets,
}

impl Table {
    const fn name(self) -> &'static str {
        match self {
            Self::Items => "items",
            Self::Tickets => "tickets",
        }
    }
}

fn version(connection: &Connection) -> Result<i64> {
    let version = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if version > 1 {
        return Err(Error::Invalid(format!(
            "state.db schema version {version} is newer than supported version 1"
        )));
    }
    Ok(version)
}

fn open(ledger: &Ledger) -> Result<Connection> {
    let directory = std::path::absolute(&ledger.dir)?;
    let wal_length = directory.join("state.db-wal").as_os_str().len();
    if wal_length >= 512 {
        return Err(Error::Invalid(format!(
            "state directory {} is {} bytes; state.db-wal path is {wal_length} bytes and must be shorter than SQLite's 512-byte limit",
            directory.display(),
            directory.as_os_str().len()
        )));
    }
    std::fs::create_dir_all(&ledger.dir)?;
    let mut connection = Connection::open(ledger.dir.join("state.db"))?;
    connection.busy_timeout(ledger.budget.remaining("state.db")?)?;
    // Refuse a newer schema before changing persistent connection settings.
    let current = version(&connection)?;
    connection.execute_batch("PRAGMA synchronous=FULL; PRAGMA foreign_keys=ON;")?;
    if current == 0 {
        // Two fresh openers deadlock upgrading SHARED to EXCLUSIVE for WAL; SQLite returns BUSY without calling the busy handler.
        ledger.with_lock("state-db", || {
            let current = version(&connection)?;
            connection.query_row("PRAGMA journal_mode=WAL", [], |_| Ok(()))?;
            if current == 0 {
                let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
                tx.execute_batch(SCHEMA)?;
                tx.commit()?;
            }
            Ok(())
        })?;
    } else {
        connection.query_row("PRAGMA journal_mode=WAL", [], |_| Ok(()))?;
    }
    Ok(connection)
}

fn classify(error: Error) -> Error {
    if let Error::Sqlite(rusqlite::Error::SqliteFailure(code, _)) = &error
        && matches!(
            code.code,
            rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
        )
    {
        return Error::Deadline("state.db busy".into());
    }
    error
}

pub(crate) fn read<T>(ledger: &Ledger, f: impl FnOnce(&Transaction<'_>) -> Result<T>) -> Result<T> {
    transact(ledger, TransactionBehavior::Deferred, f)
}

pub(crate) fn write<T>(
    ledger: &Ledger,
    f: impl FnOnce(&Transaction<'_>) -> Result<T>,
) -> Result<T> {
    transact(ledger, TransactionBehavior::Immediate, f)
}

fn transact<T>(
    ledger: &Ledger,
    behavior: TransactionBehavior,
    f: impl FnOnce(&Transaction<'_>) -> Result<T>,
) -> Result<T> {
    (|| {
        let mut connection = open(ledger)?;
        connection.busy_timeout(ledger.budget.remaining("state.db")?)?;
        let tx = connection.transaction_with_behavior(behavior)?;
        let result = f(&tx)?;
        tx.commit()?;
        Ok(result)
    })()
    .map_err(classify)
}

pub(crate) fn rows<T: DeserializeOwned>(tx: &Transaction<'_>, table: Table) -> Result<Vec<T>> {
    let mut statement = tx.prepare(&format!("SELECT record FROM {} ORDER BY seq", table.name()))?;
    let records = statement.query_map([], |row| row.get::<_, String>(0))?;
    records
        .map(|record| Ok(serde_json::from_str(&record?)?))
        .collect()
}

pub(crate) fn sync<T: Serialize>(tx: &Transaction<'_>, table: Table, records: &[T]) -> Result<()> {
    let name = table.name();
    let mut statement = tx.prepare(&format!("SELECT id, record, seq FROM {name}"))?;
    let mut stored: HashMap<String, (String, i64)> = statement
        .query_map([], |row| Ok((row.get(0)?, (row.get(1)?, row.get(2)?))))?
        .collect::<std::result::Result<_, _>>()?;
    let mut seq = stored.values().map(|(_, seq)| *seq).max().unwrap_or(0);
    for record in records {
        let record = serde_json::to_string(record)?;
        let value: serde_json::Value = serde_json::from_str(&record)?;
        let id = value["id"]
            .as_str()
            .ok_or_else(|| Error::Invalid("store record has no string id".into()))?;
        if let Some((old, _)) = stored.remove(id) {
            if old != record {
                tx.execute(
                    &format!("UPDATE {name} SET record = ?1 WHERE id = ?2"),
                    (&record, id),
                )?;
            }
        } else {
            seq += 1;
            tx.execute(
                &format!("INSERT INTO {name} (seq, record) VALUES (?1, ?2)"),
                (seq, &record),
            )?;
        }
    }
    for id in stored.keys() {
        tx.execute(&format!("DELETE FROM {name} WHERE id = ?1"), [id])?;
    }
    Ok(())
}

#[cfg(test)]
pub(crate) fn put_items(ledger: &Ledger, records: &impl Serialize) -> Result<()> {
    replace(ledger, Table::Items, records)
}

#[cfg(test)]
pub(crate) fn put_tickets(ledger: &Ledger, records: &impl Serialize) -> Result<()> {
    replace(ledger, Table::Tickets, records)
}

#[cfg(test)]
fn replace(ledger: &Ledger, table: Table, records: &impl Serialize) -> Result<()> {
    let records: Vec<serde_json::Value> = serde_json::from_value(serde_json::to_value(records)?)?;
    write(ledger, |tx| {
        tx.execute(&format!("DELETE FROM {}", table.name()), [])?;
        sync(tx, table, &records)
    })
}

#[cfg(test)]
pub(crate) fn items(ledger: &Ledger) -> Result<serde_json::Value> {
    read(ledger, |tx| {
        Ok(serde_json::to_value(rows::<serde_json::Value>(
            tx,
            Table::Items,
        )?)?)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use std::sync::{Arc, Barrier};
    use std::time::{Duration, Instant};

    fn ledger() -> Ledger {
        Ledger::new(std::env::temp_dir().join(crate::ledger::new_id()))
    }

    fn item(id: &str) -> Value {
        json!({"id":id,"host":"local","pane":id,"closed_ms":null})
    }

    fn stored(ledger: &Ledger) -> Vec<(i64, String)> {
        read(ledger, |tx| {
            Ok(tx
                .prepare("SELECT seq, record FROM items ORDER BY seq")?
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect::<std::result::Result<_, _>>()?)
        })
        .unwrap()
    }

    #[test]
    fn sync_updates_only_changed_rows_and_keeps_order() {
        let ledger = ledger();
        let mut records = vec![item("first"), item("second"), item("third")];
        write(&ledger, |tx| {
            tx.execute_batch("CREATE TABLE updates (id TEXT); CREATE TRIGGER item_updates AFTER UPDATE ON items BEGIN INSERT INTO updates VALUES (new.id); END;")?;
            sync(tx, Table::Items, &records)
        }).unwrap();
        let before = stored(&ledger);
        records[1]["closed_ms"] = json!(100);
        write(&ledger, |tx| sync(tx, Table::Items, &records)).unwrap();
        let after = stored(&ledger);
        assert_eq!(before[0], after[0]);
        assert_eq!(before[2], after[2]);
        assert_eq!(before[1].0, after[1].0);
        assert_ne!(before[1].1, after[1].1);
        assert_eq!(items(&ledger).unwrap(), json!(records));
        write(&ledger, |tx| sync(tx, Table::Items, &records)).unwrap();
        let updates: Vec<String> = read(&ledger, |tx| {
            Ok(tx
                .prepare("SELECT id FROM updates")?
                .query_map([], |row| row.get(0))?
                .collect::<std::result::Result<_, _>>()?)
        })
        .unwrap();
        assert_eq!(updates, ["second"]);
        std::fs::remove_dir_all(ledger.dir).unwrap();
    }

    #[test]
    fn sync_appends_new_ids_and_deletes_missing_ids() {
        let ledger = ledger();
        put_items(&ledger, &vec![item("first"), item("second")]).unwrap();
        let before = stored(&ledger);
        write(&ledger, |tx| {
            sync(tx, Table::Items, &[item("second"), item("third")])
        })
        .unwrap();
        let after = stored(&ledger);
        assert_eq!(before[1], after[0]);
        assert!(after[1].0 > after[0].0);
        assert_eq!(
            items(&ledger).unwrap(),
            json!([item("second"), item("third")])
        );
        std::fs::remove_dir_all(ledger.dir).unwrap();
    }

    #[test]
    fn newer_version_refuses_reads_and_writes_without_changing_database() {
        let ledger = ledger();
        std::fs::create_dir_all(&ledger.dir).unwrap();
        let path = ledger.dir.join("state.db");
        let connection = Connection::open(&path).unwrap();
        connection.execute_batch("PRAGMA user_version=2;").unwrap();
        drop(connection);
        let before = std::fs::read(&path).unwrap();
        for error in [
            read(&ledger, |_| Ok(())).unwrap_err(),
            write(&ledger, |_| Ok(())).unwrap_err(),
        ] {
            assert!(matches!(error, Error::Invalid(_)), "{error}");
        }
        assert_eq!(std::fs::read(&path).unwrap(), before);
        std::fs::remove_dir_all(ledger.dir).unwrap();
    }

    #[test]
    fn concurrent_connections_both_commit_on_a_new_database() {
        let ledger = ledger();
        let barrier = Arc::new(Barrier::new(2));
        std::thread::scope(|scope| {
            let handles: Vec<_> = (0..2)
                .map(|thread| {
                    let ledger = &ledger;
                    let barrier = Arc::clone(&barrier);
                    scope.spawn(move || {
                        barrier.wait();
                        for index in 0..10 {
                            write(ledger, |tx| {
                                let mut records: Vec<Value> = rows(tx, Table::Items)?;
                                records.push(item(&format!("{thread}-{index}")));
                                sync(tx, Table::Items, &records)
                            })
                            .unwrap();
                        }
                    })
                })
                .collect();
            for handle in handles {
                handle.join().unwrap();
            }
        });
        let records = items(&ledger).unwrap();
        assert_eq!(records.as_array().unwrap().len(), 20);
        for thread in 0..2 {
            for index in 0..10 {
                assert!(
                    records
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|record| record["id"] == format!("{thread}-{index}"))
                );
            }
        }
        std::fs::remove_dir_all(ledger.dir).unwrap();
    }

    #[test]
    fn failed_write_rolls_back_both_collections() {
        let ledger = ledger();
        let error = write(&ledger, |tx| {
            sync(tx, Table::Items, &[item("item")])?;
            sync(
                tx,
                Table::Tickets,
                &[json!({"id":"ticket","request":{"id":"request"},"state":"pending"})],
            )?;
            Err::<(), _>(Error::Invalid("abort".into()))
        })
        .unwrap_err();
        assert!(matches!(error, Error::Invalid(_)));
        assert_eq!(items(&ledger).unwrap(), json!([]));
        let tickets: Vec<Value> = read(&ledger, |tx| rows(tx, Table::Tickets)).unwrap();
        assert_eq!(tickets, [] as [Value; 0]);
        std::fs::remove_dir_all(ledger.dir).unwrap();
    }

    #[test]
    fn overlong_state_directory_refuses_before_opening_database() {
        let root = ledger();
        let prefix = std::path::absolute(&root.dir)
            .unwrap()
            .join("a".repeat(200));
        let padding = 512 - prefix.join("state.db-wal").as_os_str().len() - 1;
        let ledger = Ledger::new(prefix.join("b".repeat(padding)));
        assert_eq!(ledger.dir.join("state.db-wal").as_os_str().len(), 512);
        for error in [
            read(&ledger, |_| Ok(())).unwrap_err(),
            write(&ledger, |_| Ok(())).unwrap_err(),
        ] {
            assert!(matches!(error, Error::Invalid(_)), "{error}");
            let message = error.to_string();
            assert!(message.contains(ledger.dir.to_str().unwrap()), "{message}");
            assert!(
                message.contains(&ledger.dir.as_os_str().len().to_string()),
                "{message}"
            );
            assert!(message.contains("512-byte limit"), "{message}");
        }
        assert!(!root.dir.exists());
    }

    #[test]
    fn busy_database_uses_the_command_deadline() {
        let mut ledger = ledger();
        let mut connection = open(&ledger).unwrap();
        let tx = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        ledger.budget = crate::budget::Budget::until(Instant::now() + Duration::from_millis(50));
        let error = write(&ledger, |_| Ok(())).unwrap_err();
        assert!(
            matches!(error, Error::Deadline(ref detail) if detail == "state.db busy"),
            "{error}"
        );
        drop(tx);
        drop(connection);
        std::fs::remove_dir_all(ledger.dir).unwrap();
    }
}
