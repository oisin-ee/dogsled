//! Per-host transactional storage: the connection, the schema version, and transactions.
//! Each table's typed rows belong to its module.

use rusqlite::{Connection, Transaction, TransactionBehavior};

use crate::error::{Error, Result};
use crate::ledger::Ledger;

/// The schema version this binary creates. A store with an older version starts again
/// empty: earlier records are not read or converted.
const VERSION: i64 = 2;

/// The whole `state.db` schema.
pub(crate) fn schema() -> String {
    format!("{}{}", crate::tables::SCHEMA, crate::admissions::SCHEMA)
}

fn create(connection: &Connection) -> Result<()> {
    connection.execute_batch(&format!("{}PRAGMA user_version = {VERSION};", schema()))?;
    Ok(())
}

fn version(connection: &Connection) -> Result<i64> {
    let version = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if version > VERSION {
        return Err(Error::Invalid(format!(
            "state.db schema version {version} is newer than supported version {VERSION}"
        )));
    }
    Ok(version)
}

/// The database path, refused when SQLite could not open its WAL file there.
fn database(ledger: &Ledger) -> Result<std::path::PathBuf> {
    let directory = std::path::absolute(&ledger.dir)?;
    let wal_length = directory.join("state.db-wal").as_os_str().len();
    if wal_length >= 512 {
        return Err(Error::Invalid(format!(
            "state directory {} is {} bytes; state.db-wal path is {wal_length} bytes and must be shorter than SQLite's 512-byte limit",
            directory.display(),
            directory.as_os_str().len()
        )));
    }
    Ok(directory.join("state.db"))
}

fn open(ledger: &Ledger) -> Result<Connection> {
    let path = database(ledger)?;
    std::fs::create_dir_all(&ledger.dir)?;
    let mut connection = Connection::open(path)?;
    connection.busy_timeout(ledger.budget.remaining("state.db")?)?;
    // Refuse a newer schema before changing persistent connection settings.
    let current = version(&connection)?;
    connection.execute_batch("PRAGMA synchronous=FULL;")?;
    if current < VERSION {
        // Two fresh openers deadlock upgrading SHARED to EXCLUSIVE for WAL; SQLite returns BUSY without calling the busy handler.
        ledger.with_lock("state-db", || {
            let current = version(&connection)?;
            connection.query_row("PRAGMA journal_mode=WAL", [], |_| Ok(()))?;
            if current < VERSION {
                // An older store starts again empty. Nothing reads or converts it.
                let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
                let tables: Vec<String> = tx
                    .prepare(
                        "SELECT name FROM sqlite_schema
                         WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
                    )?
                    .query_map([], |row| row.get(0))?
                    .collect::<rusqlite::Result<_>>()?;
                for table in tables {
                    tx.execute_batch(&format!("DROP TABLE \"{table}\""))?;
                }
                create(&tx)?;
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
        let mut connection = if matches!(behavior, TransactionBehavior::Deferred)
            && !database(ledger)?.try_exists()?
        {
            // A read never creates the store: a host without one has empty tables.
            let connection = Connection::open_in_memory()?;
            create(&connection)?;
            connection
        } else {
            open(ledger)?
        };
        connection.busy_timeout(ledger.budget.remaining("state.db")?)?;
        let tx = connection.transaction_with_behavior(behavior)?;
        let result = f(&tx)?;
        tx.commit()?;
        Ok(result)
    })()
    .map_err(classify)
}

#[cfg(test)]
pub(crate) fn put_items(ledger: &Ledger, records: &impl serde::Serialize) -> Result<()> {
    let items: Vec<crate::items::Item> = serde_json::from_value(serde_json::to_value(records)?)?;
    write(ledger, |tx| {
        tx.execute("DELETE FROM items", [])?;
        crate::tables::sync_items(tx, &items)
    })
}

#[cfg(test)]
pub(crate) fn put_tickets(ledger: &Ledger, records: &impl serde::Serialize) -> Result<()> {
    let tickets: Vec<crate::work::Ticket> = serde_json::from_value(serde_json::to_value(records)?)?;
    write(ledger, |tx| {
        tx.execute("DELETE FROM tickets", [])?;
        crate::tables::sync_tickets(tx, &tickets)
    })
}

#[cfg(test)]
pub(crate) fn items(ledger: &Ledger) -> Result<serde_json::Value> {
    read(ledger, |tx| {
        Ok(serde_json::to_value(crate::tables::items(tx)?)?)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::items::Item;
    use crate::tables::{sync_items, sync_tickets};
    use serde_json::json;
    use std::sync::{Arc, Barrier};
    use std::time::{Duration, Instant};

    fn ledger() -> Ledger {
        Ledger::new(std::env::temp_dir().join(crate::ledger::new_id()))
    }

    fn item(id: &str) -> Item {
        Item {
            id: id.into(),
            kind: "agent".into(),
            host: "local".into(),
            pane: id.into(),
            owner_pane: "w1:p1".into(),
            ..Item::default()
        }
    }

    fn stored(ledger: &Ledger) -> Vec<(i64, Option<i64>)> {
        read(ledger, |tx| {
            Ok(tx
                .prepare("SELECT seq, closed_ms FROM items ORDER BY seq")?
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
            sync_items(tx, &records)
        }).unwrap();
        let before = stored(&ledger);
        records[1].closed_ms = Some(100);
        write(&ledger, |tx| sync_items(tx, &records)).unwrap();
        let after = stored(&ledger);
        assert_eq!(before[0], after[0]);
        assert_eq!(before[2], after[2]);
        assert_eq!(before[1].0, after[1].0);
        assert_eq!(after[1].1, Some(100));
        assert_eq!(items(&ledger).unwrap(), json!(records));
        write(&ledger, |tx| sync_items(tx, &records)).unwrap();
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
            sync_items(tx, &[item("second"), item("third")])
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
    fn every_item_field_survives_a_round_trip() {
        let ledger = ledger();
        let mut full = item("full");
        full.owner_session = Some("session".into());
        full.name = Some("worker".into());
        full.closed_ms = Some(9);
        full.reply = Some(crate::items::Reply {
            seq: 3,
            result_id: Some("result".into()),
            text: "FINAL".into(),
            captured_ms: 4,
            reported_ms: Some(5),
            done: true,
        });
        full.signal = Some(crate::items::ItemSignal {
            kind: crate::lifecycle::Signal::Ask,
            text: "why?".into(),
            event_id: "event".into(),
            recorded_ms: 6,
            session: None,
            reported_ms: None,
        });
        full.native_done = true;
        full.branch = Some(crate::done::Branch {
            repo: "/repo".into(),
            name: "feat/x".into(),
            ahead: 2,
            landed: false,
        });
        full.recovery = Some(crate::preservation::Recovery {
            host: "local".into(),
            repository: "/repo".into(),
            id: "recovery".into(),
            pinned_oid: "0".repeat(40),
            predecessors: vec!["1".repeat(40), "2".repeat(40)],
            operation: "close".into(),
            item: Some("full".into()),
        });
        put_items(&ledger, &vec![full.clone()]).unwrap();
        assert_eq!(items(&ledger).unwrap(), json!([full]));
        std::fs::remove_dir_all(ledger.dir).unwrap();
    }

    #[test]
    fn newer_version_refuses_reads_and_writes_without_changing_database() {
        let ledger = ledger();
        std::fs::create_dir_all(&ledger.dir).unwrap();
        let path = ledger.dir.join("state.db");
        let connection = Connection::open(&path).unwrap();
        connection.execute_batch("PRAGMA user_version=3;").unwrap();
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
    fn an_older_store_starts_again_empty() {
        let ledger = ledger();
        std::fs::create_dir_all(&ledger.dir).unwrap();
        let connection = Connection::open(ledger.dir.join("state.db")).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE items (seq INTEGER PRIMARY KEY, record TEXT);
                 INSERT INTO items (record) VALUES ('{}');
                 PRAGMA user_version=1;",
            )
            .unwrap();
        drop(connection);
        assert_eq!(items(&ledger).unwrap(), json!([]));
        write(&ledger, |tx| sync_items(tx, &[item("new")])).unwrap();
        assert_eq!(items(&ledger).unwrap(), json!([item("new")]));
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
                                let mut records = crate::tables::items(tx)?;
                                records.push(item(&format!("{thread}-{index}")));
                                sync_items(tx, &records)
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
    fn reading_an_absent_store_creates_nothing() {
        let ledger = ledger();
        assert_eq!(items(&ledger).unwrap(), json!([]));
        assert!(read(&ledger, crate::tables::tickets).unwrap().is_empty());
        assert!(!ledger.dir.exists());
    }

    #[test]
    fn failed_write_rolls_back_both_collections() {
        let ledger = ledger();
        let request = crate::admissions::fixtures::request("request", "w1:p1", None);
        let ticket: crate::work::Ticket = serde_json::from_value(json!({
            "id":"ticket","request":request,"created_ms":1,"state":"pending","waiting":false,
            "waiting_until_ms":null,"notified":false,"result":null,"error":null
        }))
        .unwrap();
        let error = write(&ledger, |tx| {
            sync_items(tx, &[item("item")])?;
            sync_tickets(tx, &[ticket])?;
            Err::<(), _>(Error::Invalid("abort".into()))
        })
        .unwrap_err();
        assert!(matches!(error, Error::Invalid(_)));
        assert_eq!(items(&ledger).unwrap(), json!([]));
        assert!(read(&ledger, crate::tables::tickets).unwrap().is_empty());
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
