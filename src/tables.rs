//! The typed `tickets` and `items` tables of `state.db`. Each list is kept in insertion
//! order by `seq`; `sync` rewrites only the rows that changed.

use std::collections::HashMap;

use rusqlite::types::Value as Sql;
use rusqlite::{Row, Transaction};
use serde::Serialize;

use crate::error::Result;
use crate::items::{Item, ItemSignal, Reply};
use crate::rows::{
    self, Columns, diagnostic_sql, list, number, request_sql, required, text, unsigned,
};
use crate::work::Ticket;

pub(crate) const SCHEMA: &str = concat!(
    "
CREATE TABLE tickets (
    seq INTEGER PRIMARY KEY,
    id TEXT NOT NULL UNIQUE,",
    request_sql!(),
    "
    finished_ms INTEGER,
    created_ms INTEGER NOT NULL,
    state TEXT NOT NULL,
    waiting INTEGER NOT NULL,
    waiting_until_ms INTEGER,
    wait_reason TEXT,
    notified INTEGER NOT NULL,
    result_message BLOB,
    error TEXT,",
    diagnostic_sql!(),
    "
    retry_attempts INTEGER NOT NULL,
    retry_next_ms INTEGER NOT NULL,
    UNIQUE (request_id)
);
CREATE INDEX tickets_state ON tickets (state);
CREATE TABLE ticket_job_args (
    ticket TEXT NOT NULL REFERENCES tickets (id) ON DELETE CASCADE ON UPDATE CASCADE,
    position INTEGER NOT NULL,
    arg TEXT NOT NULL,
    PRIMARY KEY (ticket, position)
) WITHOUT ROWID;
CREATE TABLE items (
    seq INTEGER PRIMARY KEY,
    id TEXT NOT NULL UNIQUE,
    kind TEXT NOT NULL,
    host TEXT NOT NULL,
    pane TEXT NOT NULL,
    owner_pane TEXT NOT NULL,
    owner_session TEXT,
    name TEXT,
    created_ms INTEGER NOT NULL,
    launch_confirmed_ms INTEGER,
    closed_ms INTEGER,
    job_ended_ms INTEGER,
    dropped_ms INTEGER,
    branch_checked_ms INTEGER,
    harness TEXT,
    profile TEXT,
    model TEXT,
    role TEXT,
    reply_seq INTEGER,
    reply_result_id TEXT,
    reply_text TEXT,
    reply_captured_ms INTEGER,
    reply_reported_ms INTEGER,
    reply_done INTEGER,
    orphan_reply_seq INTEGER,
    orphan_reply_result_id TEXT,
    orphan_reply_text TEXT,
    orphan_reply_captured_ms INTEGER,
    orphan_reply_reported_ms INTEGER,
    orphan_reply_done INTEGER,
    resource_closed_ms INTEGER,
    resource_close_reason TEXT,
    signal_kind TEXT CHECK (signal_kind IN ('ask', 'blocked')),
    signal_text TEXT,
    signal_event_id TEXT,
    signal_recorded_ms INTEGER,
    signal_session TEXT,
    signal_reported_ms INTEGER,
    native_done INTEGER NOT NULL,
    branch_repo TEXT,
    branch_name TEXT,
    branch_ahead INTEGER,
    branch_landed INTEGER,
    recovery_host TEXT,
    recovery_repository TEXT,
    recovery_id TEXT,
    recovery_pinned_oid TEXT,
    recovery_operation TEXT,
    recovery_item TEXT,
    CHECK ((reply_seq IS NULL) = (reply_text IS NULL)),
    CHECK ((orphan_reply_seq IS NULL) = (orphan_reply_text IS NULL)),
    CHECK ((signal_kind IS NULL) = (signal_event_id IS NULL)),
    CHECK ((branch_repo IS NULL) = (branch_name IS NULL)),
    CHECK ((recovery_id IS NULL) = (recovery_pinned_oid IS NULL))
);
CREATE INDEX items_open_pane ON items (host, pane) WHERE closed_ms IS NULL;
CREATE TABLE item_recovery_predecessors (
    item TEXT NOT NULL REFERENCES items (id) ON DELETE CASCADE ON UPDATE CASCADE,
    position INTEGER NOT NULL,
    oid TEXT NOT NULL,
    PRIMARY KEY (item, position)
) WITHOUT ROWID;
"
);

/// Service diagnostics are independent of the execution records and start empty.
pub(crate) const SERVICE_LANES_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS service_lanes (
    lane TEXT PRIMARY KEY NOT NULL,
    error TEXT,
    since_ms INTEGER,
    last_ms INTEGER NOT NULL,
    repeats INTEGER NOT NULL,
    CHECK ((error IS NULL) = (since_ms IS NULL))
);
";

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, Serialize)]
pub struct ServiceLane {
    pub lane: String,
    pub error: Option<String>,
    pub since_ms: Option<u64>,
    pub last_ms: u64,
    pub repeats: u64,
}

fn service_lane(row: &Row<'_>) -> Result<ServiceLane> {
    Ok(ServiceLane {
        lane: required(row, "lane")?,
        error: text(row, "error")?,
        since_ms: number(row, "since_ms")?,
        last_ms: unsigned(row.get("last_ms")?)?,
        repeats: unsigned(row.get("repeats")?)?,
    })
}

pub(crate) fn failing_lanes(tx: &Transaction<'_>) -> Result<Vec<ServiceLane>> {
    let mut statement =
        tx.prepare_cached("SELECT * FROM service_lanes WHERE error IS NOT NULL ORDER BY lane")?;
    let mut rows = statement.query([])?;
    let mut lanes = Vec::new();
    while let Some(row) = rows.next()? {
        lanes.push(service_lane(row)?);
    }
    Ok(lanes)
}

/// Return whether the diagnostic changed, in the same transaction that records the pass.
pub(crate) fn record_lane(
    tx: &Transaction<'_>,
    lane: &str,
    error: Option<&str>,
    now: u64,
) -> Result<bool> {
    let mut statement = tx.prepare_cached("SELECT * FROM service_lanes WHERE lane = ?1")?;
    let mut rows = statement.query([lane])?;
    let previous = rows.next()?.map(service_lane).transpose()?;
    let changed = previous.as_ref().and_then(|old| old.error.as_deref()) != error;
    let since = error.map(|_| {
        if changed {
            now
        } else {
            previous
                .as_ref()
                .and_then(|old| old.since_ms)
                .unwrap_or(now)
        }
    });
    let repeats = if error.is_none() {
        0
    } else if changed {
        1
    } else {
        previous
            .as_ref()
            .map_or(1, |old| old.repeats.saturating_add(1))
    };
    let mut columns = Columns::default();
    columns.text("lane", Some(lane));
    columns.text("error", error);
    columns.number("since_ms", since)?;
    columns.number("last_ms", Some(now))?;
    columns.number("repeats", Some(repeats))?;
    columns.upsert(tx, "service_lanes", "lane")?;
    Ok(changed)
}

pub(crate) fn tickets(tx: &Transaction<'_>) -> Result<Vec<Ticket>> {
    let mut statement = tx.prepare_cached("SELECT * FROM tickets ORDER BY seq")?;
    let mut rows = statement.query([])?;
    let mut tickets = Vec::new();
    while let Some(row) = rows.next()? {
        tickets.push(ticket_row(tx, row)?);
    }
    Ok(tickets)
}

/// One ticket by its id, without reading the rest of the queue.
pub(crate) fn ticket(tx: &Transaction<'_>, id: &str) -> Result<Option<Ticket>> {
    let mut statement = tx.prepare_cached("SELECT * FROM tickets WHERE id = ?1")?;
    let mut rows = statement.query([id])?;
    match rows.next()? {
        Some(row) => Ok(Some(ticket_row(tx, row)?)),
        None => Ok(None),
    }
}

/// Whether a ticket was queued for request `id`.
pub(crate) fn queued_request(tx: &Transaction<'_>, id: &str) -> Result<bool> {
    Ok(tx
        .prepare_cached("SELECT 1 FROM tickets WHERE request_id = ?1")?
        .exists([id])?)
}

/// Make the stored queue equal `tickets`, in their order of first insertion.
pub(crate) fn sync_tickets(tx: &Transaction<'_>, tickets: &[Ticket]) -> Result<()> {
    let stored = self::tickets(tx)?;
    sync(
        tx,
        "tickets",
        &stored,
        tickets,
        |ticket| &ticket.id,
        save_ticket,
    )
}

pub(crate) fn items(tx: &Transaction<'_>) -> Result<Vec<Item>> {
    let mut statement = tx.prepare_cached("SELECT * FROM items ORDER BY seq")?;
    let mut rows = statement.query([])?;
    let mut items = Vec::new();
    while let Some(row) = rows.next()? {
        items.push(item_row(tx, row)?);
    }
    Ok(items)
}

/// Make the stored items equal `items`, in their order of first insertion.
pub(crate) fn sync_items(tx: &Transaction<'_>, items: &[Item]) -> Result<()> {
    let stored = self::items(tx)?;
    sync(tx, "items", &stored, items, |item| &item.id, save_item)
}

fn sync<T: Serialize>(
    tx: &Transaction<'_>,
    table: &str,
    stored: &[T],
    records: &[T],
    id: impl Fn(&T) -> &String,
    save: impl Fn(&Transaction<'_>, &T) -> Result<()>,
) -> Result<()> {
    let mut stored: HashMap<&str, Vec<u8>> = stored
        .iter()
        .map(|record| Ok((id(record).as_str(), serde_json::to_vec(record)?)))
        .collect::<Result<_>>()?;
    for record in records {
        match stored.remove(id(record).as_str()) {
            Some(old) if old == serde_json::to_vec(record)? => {}
            _ => save(tx, record)?,
        }
    }
    let mut delete = tx.prepare_cached(&format!("DELETE FROM {table} WHERE id = ?1"))?;
    for id in stored.keys() {
        delete.execute([id])?;
    }
    Ok(())
}

fn save_ticket(tx: &Transaction<'_>, ticket: &Ticket) -> Result<()> {
    let mut columns = Columns::default();
    columns.text("id", Some(&ticket.id));
    let args = rows::request_columns(&mut columns, &ticket.request)?;
    columns.number("finished_ms", ticket.finished_ms)?;
    columns.number("created_ms", Some(ticket.created_ms))?;
    columns.text("state", Some(&ticket.state));
    columns.flag("waiting", ticket.waiting);
    columns.number("waiting_until_ms", ticket.waiting_until_ms)?;
    columns.text("wait_reason", ticket.wait_reason.as_deref());
    columns.flag("notified", ticket.notified);
    columns.message("result_message", ticket.result.as_ref())?;
    columns.text("error", ticket.error.as_deref());
    rows::diagnostic_columns(&mut columns, ticket.diagnostic.as_ref())?;
    columns.number("retry_attempts", Some(u64::from(ticket.retry.attempts)))?;
    columns.number("retry_next_ms", Some(ticket.retry.next_ms))?;
    columns.upsert(tx, "tickets", "id")?;
    rows::replace_list(
        tx,
        "ticket_job_args",
        "ticket",
        "arg",
        &ticket.id,
        args.into_iter().map(Sql::Text),
    )
}

fn ticket_row(tx: &Transaction<'_>, row: &Row<'_>) -> Result<Ticket> {
    let id = required(row, "id")?;
    let request = rows::load_request(row, || {
        list(
            tx,
            "SELECT arg FROM ticket_job_args WHERE ticket = ?1 ORDER BY position",
            &id,
            |row| row.get(0),
        )
    })?;
    Ok(Ticket {
        finished_ms: number(row, "finished_ms")?,
        request,
        created_ms: unsigned(row.get("created_ms")?)?,
        state: required(row, "state")?,
        waiting: row.get("waiting")?,
        waiting_until_ms: number(row, "waiting_until_ms")?,
        wait_reason: text(row, "wait_reason")?,
        notified: row.get("notified")?,
        result: rows::decoded(row, "result_message")?,
        error: text(row, "error")?,
        diagnostic: rows::load_diagnostic(row)?,
        retry: crate::backoff::Backoff {
            attempts: u32::try_from(row.get::<_, i64>("retry_attempts")?)
                .map_err(|_| crate::error::Error::Invalid("retry attempts".into()))?,
            next_ms: unsigned(row.get("retry_next_ms")?)?,
        },
        id,
    })
}

fn reply_columns(
    columns: &mut Columns,
    prefix: [&'static str; 6],
    reply: Option<&Reply>,
) -> Result<()> {
    let Some(reply) = reply else {
        return Ok(());
    };
    let [seq, result_id, text, captured_ms, reported_ms, done] = prefix;
    columns.number(seq, Some(reply.seq))?;
    columns.text(result_id, reply.result_id.as_deref());
    columns.text(text, Some(&reply.text));
    columns.number(captured_ms, Some(reply.captured_ms))?;
    columns.number(reported_ms, reply.reported_ms)?;
    columns.flag(done, reply.done);
    Ok(())
}

const REPLY: [&str; 6] = [
    "reply_seq",
    "reply_result_id",
    "reply_text",
    "reply_captured_ms",
    "reply_reported_ms",
    "reply_done",
];
const ORPHAN_REPLY: [&str; 6] = [
    "orphan_reply_seq",
    "orphan_reply_result_id",
    "orphan_reply_text",
    "orphan_reply_captured_ms",
    "orphan_reply_reported_ms",
    "orphan_reply_done",
];

fn reply_row(row: &Row<'_>, prefix: [&str; 6]) -> Result<Option<Reply>> {
    let [seq, result_id, text_column, captured_ms, reported_ms, done] = prefix;
    let Some(seq) = number(row, seq)? else {
        return Ok(None);
    };
    Ok(Some(Reply {
        seq,
        result_id: text(row, result_id)?,
        text: required(row, text_column)?,
        captured_ms: number(row, captured_ms)?.unwrap_or_default(),
        reported_ms: number(row, reported_ms)?,
        done: row.get::<_, Option<bool>>(done)?.unwrap_or_default(),
    }))
}

fn save_item(tx: &Transaction<'_>, item: &Item) -> Result<()> {
    let mut columns = Columns::default();
    columns.text("id", Some(&item.id));
    columns.text("kind", Some(&item.kind));
    columns.text("host", Some(&item.host));
    columns.text("pane", Some(&item.pane));
    columns.text("owner_pane", Some(&item.owner_pane));
    columns.text("owner_session", item.owner_session.as_deref());
    columns.text("name", item.name.as_deref());
    columns.number("created_ms", Some(item.created_ms))?;
    columns.number("launch_confirmed_ms", item.launch_confirmed_ms)?;
    columns.number("closed_ms", item.closed_ms)?;
    columns.number("job_ended_ms", item.job_ended_ms)?;
    columns.number("dropped_ms", item.dropped_ms)?;
    columns.number("branch_checked_ms", item.branch_checked_ms)?;
    columns.text("harness", item.harness.as_deref());
    columns.text("profile", item.profile.as_deref());
    columns.text("model", item.model.as_deref());
    columns.text("role", item.role.as_deref());
    reply_columns(&mut columns, REPLY, item.reply.as_ref())?;
    reply_columns(&mut columns, ORPHAN_REPLY, item.orphan_reply.as_ref())?;
    columns.number("resource_closed_ms", item.resource_closed_ms)?;
    columns.text(
        "resource_close_reason",
        item.resource_close_reason.as_deref(),
    );
    if let Some(signal) = &item.signal {
        columns.text("signal_kind", Some(signal.kind.as_str()));
        columns.text("signal_text", Some(&signal.text));
        columns.text("signal_event_id", Some(&signal.event_id));
        columns.number("signal_recorded_ms", Some(signal.recorded_ms))?;
        columns.text("signal_session", signal.session.as_deref());
        columns.number("signal_reported_ms", signal.reported_ms)?;
    }
    columns.flag("native_done", item.native_done);
    if let Some(branch) = &item.branch {
        columns.text("branch_repo", Some(&branch.repo));
        columns.text("branch_name", Some(&branch.name));
        columns.number("branch_ahead", Some(branch.ahead))?;
        columns.flag("branch_landed", branch.landed);
    }
    let mut predecessors = Vec::new();
    if let Some(recovery) = &item.recovery {
        columns.text("recovery_host", Some(&recovery.host));
        columns.text("recovery_repository", Some(&recovery.repository));
        columns.text("recovery_id", Some(&recovery.id));
        columns.text("recovery_pinned_oid", Some(&recovery.pinned_oid));
        columns.text("recovery_operation", Some(&recovery.operation));
        columns.text("recovery_item", recovery.item.as_deref());
        predecessors.clone_from(&recovery.predecessors);
    }
    columns.upsert(tx, "items", "id")?;
    rows::replace_list(
        tx,
        "item_recovery_predecessors",
        "item",
        "oid",
        &item.id,
        predecessors.into_iter().map(Sql::Text),
    )
}

fn item_row(tx: &Transaction<'_>, row: &Row<'_>) -> Result<Item> {
    let id = required(row, "id")?;
    let signal = match text(row, "signal_event_id")? {
        Some(event_id) => Some(ItemSignal {
            kind: rows::variant(required(row, "signal_kind")?)?,
            text: required(row, "signal_text")?,
            event_id,
            recorded_ms: number(row, "signal_recorded_ms")?.unwrap_or_default(),
            session: text(row, "signal_session")?,
            reported_ms: number(row, "signal_reported_ms")?,
        }),
        None => None,
    };
    let branch = match text(row, "branch_repo")? {
        Some(repo) => Some(crate::done::Branch {
            repo,
            name: required(row, "branch_name")?,
            ahead: number(row, "branch_ahead")?.unwrap_or_default(),
            landed: row
                .get::<_, Option<bool>>("branch_landed")?
                .unwrap_or_default(),
        }),
        None => None,
    };
    let recovery = match text(row, "recovery_id")? {
        Some(recovery_id) => Some(crate::preservation::Recovery {
            host: required(row, "recovery_host")?,
            repository: required(row, "recovery_repository")?,
            id: recovery_id,
            pinned_oid: required(row, "recovery_pinned_oid")?,
            predecessors: list(
                tx,
                "SELECT oid FROM item_recovery_predecessors WHERE item = ?1 ORDER BY position",
                &id,
                |row| row.get(0),
            )?,
            operation: required(row, "recovery_operation")?,
            item: text(row, "recovery_item")?,
        }),
        None => None,
    };
    Ok(Item {
        kind: required(row, "kind")?,
        host: required(row, "host")?,
        pane: required(row, "pane")?,
        owner_pane: required(row, "owner_pane")?,
        owner_session: text(row, "owner_session")?,
        name: text(row, "name")?,
        created_ms: number(row, "created_ms")?.unwrap_or_default(),
        launch_confirmed_ms: number(row, "launch_confirmed_ms")?,
        closed_ms: number(row, "closed_ms")?,
        job_ended_ms: number(row, "job_ended_ms")?,
        dropped_ms: number(row, "dropped_ms")?,
        branch_checked_ms: number(row, "branch_checked_ms")?,
        harness: text(row, "harness")?,
        profile: text(row, "profile")?,
        model: text(row, "model")?,
        role: text(row, "role")?,
        reply: reply_row(row, REPLY)?,
        orphan_reply: reply_row(row, ORPHAN_REPLY)?,
        resource_closed_ms: number(row, "resource_closed_ms")?,
        resource_close_reason: text(row, "resource_close_reason")?,
        signal,
        native_done: row.get("native_done")?,
        branch,
        recovery,
        id,
    })
}

#[cfg(test)]
mod service_lane_tests {
    use super::*;

    #[test]
    fn changed_error_starts_a_new_failure_and_success_clears_it() {
        let mut db = rusqlite::Connection::open_in_memory().unwrap();
        db.execute_batch(SERVICE_LANES_SCHEMA).unwrap();
        let tx = db.transaction().unwrap();
        assert!(!record_lane(&tx, "admission", None, 1).unwrap());
        assert_eq!(failing_lanes(&tx).unwrap(), [] as [ServiceLane; 0]);
        assert!(record_lane(&tx, "admission", Some("first"), 10).unwrap());
        assert!(!record_lane(&tx, "admission", Some("first"), 20).unwrap());
        assert_eq!(
            failing_lanes(&tx).unwrap(),
            vec![ServiceLane {
                lane: "admission".into(),
                error: Some("first".into()),
                since_ms: Some(10),
                last_ms: 20,
                repeats: 2,
            }]
        );
        assert!(record_lane(&tx, "admission", Some("changed"), 30).unwrap());
        assert_eq!(
            failing_lanes(&tx).unwrap(),
            vec![ServiceLane {
                lane: "admission".into(),
                error: Some("changed".into()),
                since_ms: Some(30),
                last_ms: 30,
                repeats: 1,
            }]
        );
        assert!(record_lane(&tx, "admission", None, 40).unwrap());
        assert!(!record_lane(&tx, "admission", None, 50).unwrap());
        assert_eq!(failing_lanes(&tx).unwrap(), [] as [ServiceLane; 0]);
        assert!(record_lane(&tx, "admission", Some("changed"), 60).unwrap());
        assert_eq!(failing_lanes(&tx).unwrap()[0].since_ms, Some(60));
        assert_eq!(failing_lanes(&tx).unwrap()[0].repeats, 1);
    }
}
