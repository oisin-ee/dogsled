//! The hidden CLI seam used by fixtures and local state inspection.

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use super::{board, inbox, questions, reviews};
use crate::cli::StateCommand;
use crate::error::{Error, Result};
use crate::ledger::Ledger;

fn input<T: DeserializeOwned>() -> Result<T> {
    Ok(serde_json::from_reader(std::io::stdin().lock())?)
}

fn value<T: Serialize>(record: T) -> Result<Value> {
    Ok(serde_json::to_value(record)?)
}

fn inbox_state(ledger: &Ledger, command: StateCommand) -> Result<Value> {
    match command {
        StateCommand::Inbox {
            terminal,
            put,
            delete,
        } => {
            crate::inbox::validate_terminal(&terminal)?;
            if put {
                let records = input::<Vec<crate::inbox::Record>>()?;
                crate::store::write(ledger, |tx| inbox::put_records(tx, &terminal, &records))?;
            } else if delete {
                crate::store::write(ledger, |tx| inbox::remove_terminal(tx, &terminal))?;
            }
            value(crate::store::read(ledger, |tx| {
                inbox::records(tx, &terminal)
            })?)
        }
        StateCommand::InboxReader {
            terminal,
            put,
            delete,
        } => {
            crate::inbox::validate_terminal(&terminal)?;
            if put {
                let record = input::<crate::inbox::Reader>()?;
                crate::store::write(ledger, |tx| inbox::put_reader(tx, &terminal, &record))?;
            } else if delete {
                crate::store::write(ledger, |tx| inbox::remove_reader(tx, &terminal))?;
            }
            value(crate::store::read(ledger, |tx| {
                inbox::reader(tx, &terminal)
            })?)
        }
        StateCommand::Outbox { put, delete } => {
            if put || delete {
                let records = if put { input()? } else { Vec::new() };
                crate::store::write(ledger, |tx| inbox::put_sent(tx, &records))?;
            }
            value(crate::store::read(ledger, inbox::sent)?)
        }
        StateCommand::NotificationFailures { put } => {
            if put {
                let records = input::<
                    std::collections::BTreeMap<String, crate::inbox::NotificationFailure>,
                >()?;
                crate::store::write(ledger, |tx| {
                    tx.execute("DELETE FROM notification_failures", [])?;
                    for (id, failure) in &records {
                        inbox::put_failure(tx, id, failure)?;
                    }
                    Ok(())
                })?;
            }
            value(crate::store::read(ledger, inbox::failures)?)
        }
        _ => Err(Error::Invalid("not an inbox state command".into())),
    }
}

pub(crate) fn run(ledger: &Ledger, command: StateCommand) -> Result<Value> {
    match command {
        command @ (StateCommand::Inbox { .. }
        | StateCommand::InboxReader { .. }
        | StateCommand::Outbox { .. }
        | StateCommand::NotificationFailures { .. }) => inbox_state(ledger, command),
        StateCommand::Todo {
            pane,
            lane,
            put,
            delete,
        } => {
            if !matches!(lane.as_str(), "published" | "cached") {
                return Err(Error::Invalid("invalid todo lane".into()));
            }
            if put {
                let todo = input::<crate::board::TodoSections>()?;
                if todo.pane != pane {
                    return Err(Error::Invalid("todo pane mismatch".into()));
                }
                crate::store::write(ledger, |tx| board::put_todo(tx, &lane, &todo))?;
            } else if delete {
                crate::store::write(ledger, |tx| board::remove_todo(tx, &lane, &pane))?;
            }
            value(crate::store::read(ledger, |tx| {
                board::todo(tx, &lane, &pane)
            })?)
        }
        StateCommand::Sidebar { owner, put, delete } => {
            if put {
                let saved = input()?;
                crate::store::write(ledger, |tx| board::put_sidebar(tx, &owner, &saved))?;
            } else if delete {
                crate::store::write(ledger, |tx| board::remove_sidebar(tx, &owner))?;
            }
            value(crate::store::read(ledger, |tx| board::sidebar(tx, &owner))?)
        }
        StateCommand::Dock { tab, put, delete } => {
            if put {
                let dock = input()?;
                crate::store::write(ledger, |tx| board::put_dock(tx, &tab, &dock))?;
            } else if delete {
                crate::store::write(ledger, |tx| board::remove_dock(tx, &tab))?;
            }
            value(crate::store::read(ledger, |tx| board::dock(tx, &tab))?)
        }
        StateCommand::Question { id, put, delete } => {
            if put {
                let entry = input::<crate::web::store::Entry>()?;
                if entry.pending.id != id {
                    return Err(Error::Invalid("question id mismatch".into()));
                }
                crate::store::write(ledger, |tx| questions::put(tx, &entry))?;
            } else if delete {
                crate::store::write(ledger, |tx| questions::remove(tx, &id))?;
            }
            value(crate::store::read(ledger, |tx| questions::get(tx, &id))?)
        }
        StateCommand::Review { id, put, delete } => {
            if put {
                let record = input::<crate::web::reviews::Record>()?;
                if record.review.id != id {
                    return Err(Error::Invalid("review id mismatch".into()));
                }
                crate::store::write(ledger, |tx| reviews::put(tx, &record))?;
            } else if delete {
                crate::store::write(ledger, |tx| reviews::remove(tx, &id))?;
            }
            value(crate::store::read(ledger, |tx| reviews::get(tx, &id))?)
        }
        StateCommand::Attachment { name, put, delete } => {
            if put {
                let record = input::<reviews::Attachment>()?;
                if record.name != name {
                    return Err(Error::Invalid("attachment name mismatch".into()));
                }
                crate::store::write(ledger, |tx| reviews::put_attachment(tx, &record))?;
            } else if delete {
                crate::store::write(ledger, |tx| reviews::remove_attachment(tx, &name))?;
            }
            value(crate::store::read(ledger, |tx| {
                reviews::attachment(tx, &name)
            })?)
        }
        _ => Err(Error::Invalid("not a web state command".into())),
    }
}
