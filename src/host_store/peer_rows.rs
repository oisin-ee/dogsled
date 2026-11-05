//! Relational peer handoff and resource-hold collections.
use super::{Columns, Result, Sql, Transaction, number, optional_number, rows};

pub(super) const SCHEMA: &str = r"CREATE TABLE host_peer_handoffs (
 owner TEXT NOT NULL REFERENCES host_peer(row_key) ON DELETE CASCADE,
 position INTEGER NOT NULL CHECK(position >= 0),
 row_key TEXT NOT NULL UNIQUE,
 pane TEXT NOT NULL,
 item TEXT,
 session TEXT,
 name TEXT,
 after INTEGER CHECK (after >= 0),
 sealed INTEGER CHECK (sealed >= 0),
 marked_ms INTEGER NOT NULL CHECK (marked_ms >= 0),
 done_turn_present INTEGER NOT NULL CHECK(done_turn_present IN (0,1)),
 done_turn_state_change_seq INTEGER CHECK (done_turn_state_change_seq >= 0),
 done_turn_state_since_ms INTEGER CHECK (done_turn_state_since_ms >= 0),
 signal_present INTEGER NOT NULL CHECK(signal_present IN (0,1)),
 signal_kind TEXT CHECK (signal_kind IN ('ask','blocked','waiting')),
 signal_text TEXT,
 signal_event_id TEXT,
 signal_turn_state_change_seq INTEGER CHECK (signal_turn_state_change_seq >= 0),
 signal_turn_state_since_ms INTEGER CHECK (signal_turn_state_since_ms >= 0),
 signal_session TEXT,
 signal_recorded_ms INTEGER CHECK (signal_recorded_ms >= 0),
 signal_cleared_ms INTEGER CHECK (signal_cleared_ms >= 0),
 signal_history_message BLOB,
 signal_inputs_message BLOB,
 signal_delivered INTEGER NOT NULL CHECK(signal_delivered IN (0,1)),
 branch_present INTEGER NOT NULL CHECK(branch_present IN (0,1)),
 branch_repo TEXT,
 branch_name TEXT,
 branch_ahead INTEGER CHECK (branch_ahead >= 0),
 branch_landed INTEGER CHECK (branch_landed IN (0,1)),
 detached INTEGER NOT NULL CHECK (detached IN (0,1)),
 reply_present INTEGER NOT NULL CHECK(reply_present IN (0,1)),
 reply_seq INTEGER CHECK (reply_seq >= 0),
 reply_result_id TEXT,
 reply_text TEXT,
 reply_captured_ms INTEGER CHECK (reply_captured_ms >= 0),
 reply_reported_ms INTEGER CHECK (reply_reported_ms >= 0),
 reply_done INTEGER CHECK (reply_done IN (0,1)),
 native_done INTEGER NOT NULL CHECK (native_done IN (0,1)),
 closed_ms INTEGER CHECK (closed_ms >= 0),
 dropped_ms INTEGER CHECK (dropped_ms >= 0),
 PRIMARY KEY(owner,position)
) STRICT;
CREATE INDEX host_peer_handoffs_owner ON host_peer_handoffs(owner,position);
CREATE INDEX host_peer_handoffs_identity ON host_peer_handoffs(owner,pane,item,session);
CREATE TABLE host_peer_holds (
 row_key TEXT PRIMARY KEY CHECK(length(row_key)>0),
 owner TEXT NOT NULL REFERENCES host_peer(row_key) ON DELETE CASCADE,
 position INTEGER NOT NULL CHECK(position>=0),
 host TEXT NOT NULL, resource TEXT NOT NULL, hold_owner TEXT NOT NULL,
 owner_pane TEXT, code TEXT NOT NULL, reason TEXT NOT NULL,
 next_command TEXT NOT NULL, command_host TEXT NOT NULL, request_id TEXT,
 UNIQUE(owner,position)
) STRICT;
CREATE INDEX host_peer_holds_owner ON host_peer_holds(owner,position);
CREATE TABLE host_peer_hold_argv (
 owner TEXT NOT NULL REFERENCES host_peer_holds(row_key) ON DELETE CASCADE,
 position INTEGER NOT NULL CHECK(position>=0),
 value TEXT NOT NULL,
 PRIMARY KEY(owner,position)
) STRICT;
CREATE TABLE host_peer_service_lanes (
 owner TEXT NOT NULL REFERENCES host_peer(row_key) ON DELETE CASCADE,
 position INTEGER NOT NULL CHECK(position>=0),
 lane TEXT NOT NULL,
 error TEXT,
 since_ms INTEGER CHECK (since_ms >= 0),
 last_ms INTEGER NOT NULL CHECK (last_ms >= 0),
 repeats INTEGER NOT NULL CHECK (repeats >= 0),
 PRIMARY KEY(owner,position),
 CHECK ((error IS NULL) = (since_ms IS NULL))
) STRICT;
";
/// Replace the service lanes a peer reported.
pub(super) fn put_service_lanes(
    tx: &Transaction<'_>,
    owner: &str,
    lanes: &[crate::tables::ServiceLane],
) -> Result<()> {
    tx.execute(
        "DELETE FROM host_peer_service_lanes WHERE owner=?1",
        [owner],
    )?;
    for (position, lane) in (0_i64..).zip(lanes) {
        tx.execute(
            "INSERT INTO host_peer_service_lanes(owner,position,lane,error,since_ms,last_ms,repeats)
             VALUES (?1,?2,?3,?4,?5,?6,?7)",
            rusqlite::params![
                owner,
                position,
                lane.lane,
                lane.error,
                lane.since_ms.map(rows::integer).transpose()?,
                rows::integer(lane.last_ms)?,
                rows::integer(lane.repeats)?,
            ],
        )?;
    }
    Ok(())
}
/// The service lanes a peer reported, in its order.
pub(super) fn service_lanes(
    tx: &Transaction<'_>,
    owner: &str,
) -> Result<Vec<crate::tables::ServiceLane>> {
    let mut statement = tx
        .prepare_cached("SELECT * FROM host_peer_service_lanes WHERE owner=?1 ORDER BY position")?;
    let mut cursor = statement.query([owner])?;
    let mut lanes = Vec::new();
    while let Some(row) = cursor.next()? {
        lanes.push(crate::tables::ServiceLane {
            lane: rows::required(row, "lane")?,
            error: rows::text(row, "error")?,
            since_ms: optional_number(row, "since_ms")?,
            last_ms: number(row, "last_ms")?,
            repeats: number(row, "repeats")?,
        });
    }
    Ok(lanes)
}
pub(super) fn put_handoff(
    tx: &Transaction<'_>,
    owner: &str,
    position: i64,
    key: &str,
    value: &crate::done::Mark,
) -> Result<()> {
    let id = format!("{}:{owner}/{position}", owner.len());
    let id = id.as_str();
    let _ = key;
    let mut columns = Columns::default();
    columns.text("row_key", Some(id));
    columns.text("owner", Some(owner));
    columns.put("position", Sql::Integer(position));
    columns.text("pane", Some(&value.pane));
    if let Some(value) = &value.item {
        columns.text("item", Some(value));
    }
    if let Some(value) = &value.session {
        columns.text("session", Some(value));
    }
    if let Some(value) = &value.name {
        columns.text("name", Some(value));
    }
    if let Some(value) = &value.after {
        columns.number("after", Some(*value))?;
    }
    if let Some(value) = &value.sealed {
        columns.number("sealed", Some(*value))?;
    }
    columns.number("marked_ms", Some(value.marked_ms))?;
    columns.flag("done_turn_present", value.done_turn.is_some());
    if let Some(value) = &value.done_turn {
        if let Some(value) = &value.state_change_seq {
            columns.number("done_turn_state_change_seq", Some(*value))?;
        }
        if let Some(value) = &value.state_since_ms {
            columns.number("done_turn_state_since_ms", Some(*value))?;
        }
    }
    columns.flag("signal_present", value.signal.is_some());
    if let Some(value) = &value.signal {
        columns.text("signal_kind", Some(&rows::name(&value.kind)?));
        columns.text("signal_text", Some(&value.text));
        columns.text("signal_event_id", Some(&value.event_id));
        if let Some(value) = &value.turn.state_change_seq {
            columns.number("signal_turn_state_change_seq", Some(*value))?;
        }
        if let Some(value) = &value.turn.state_since_ms {
            columns.number("signal_turn_state_since_ms", Some(*value))?;
        }
        columns.text("signal_session", Some(&value.session));
        columns.number("signal_recorded_ms", Some(value.recorded_ms))?;
        if let Some(value) = &value.cleared_ms {
            columns.number("signal_cleared_ms", Some(*value))?;
        }
    }
    columns.message("signal_history_message", Some(&value.signal_history))?;
    columns.message("signal_inputs_message", Some(&value.signal_inputs))?;
    columns.flag("signal_delivered", value.signal_delivered);
    columns.flag("branch_present", value.branch.is_some());
    if let Some(value) = &value.branch {
        columns.text("branch_repo", Some(&value.repo));
        columns.text("branch_name", Some(&value.name));
        columns.number("branch_ahead", Some(value.ahead))?;
        columns.flag("branch_landed", value.landed);
    }
    columns.flag("detached", value.detached);
    columns.flag("reply_present", value.reply.is_some());
    if let Some(value) = &value.reply {
        columns.number("reply_seq", Some(value.seq))?;
        if let Some(value) = &value.result_id {
            columns.text("reply_result_id", Some(value));
        }
        columns.text("reply_text", Some(&value.text));
        columns.number("reply_captured_ms", Some(value.captured_ms))?;
        if let Some(value) = &value.reported_ms {
            columns.number("reply_reported_ms", Some(*value))?;
        }
        columns.flag("reply_done", value.done);
    }
    columns.flag("native_done", value.native_done);
    if let Some(value) = &value.closed_ms {
        columns.number("closed_ms", Some(*value))?;
    }
    if let Some(value) = &value.dropped_ms {
        columns.number("dropped_ms", Some(*value))?;
    }
    columns.upsert(tx, "host_peer_handoffs", "row_key")?;
    Ok(())
}
pub(super) fn handoffs(
    tx: &Transaction<'_>,
    owner: &str,
) -> Result<Vec<(i64, String, crate::done::Mark)>> {
    let mut statement =
        tx.prepare_cached("SELECT * FROM host_peer_handoffs WHERE owner=?1 ORDER BY position")?;
    let mut cursor = statement.query([owner])?;
    let mut values = Vec::new();
    while let Some(row) = cursor.next()? {
        let id = rows::required(row, "row_key")?;
        let id = id.as_str();
        let _ = (tx, id, row);
        values.push((
            row.get("position")?,
            String::new(),
            crate::done::Mark {
                pane: rows::required(row, "pane")?,
                item: rows::text(row, "item")?,
                session: rows::text(row, "session")?,
                name: rows::text(row, "name")?,
                after: optional_number::<u64>(row, "after")?,
                sealed: optional_number::<u64>(row, "sealed")?,
                marked_ms: number::<u64>(row, "marked_ms")?,
                done_turn: if row.get::<_, Option<bool>>("done_turn_present")? == Some(true) {
                    Some(crate::done::Turn {
                        state_change_seq: optional_number::<u64>(
                            row,
                            "done_turn_state_change_seq",
                        )?,
                        state_since_ms: optional_number::<u64>(row, "done_turn_state_since_ms")?,
                    })
                } else {
                    None
                },
                signal: if row.get::<_, Option<bool>>("signal_present")? == Some(true) {
                    Some(crate::done::Signal {
                        kind: rows::variant(rows::required(row, "signal_kind")?)?,
                        text: rows::required(row, "signal_text")?,
                        event_id: rows::required(row, "signal_event_id")?,
                        turn: crate::done::Turn {
                            state_change_seq: optional_number::<u64>(
                                row,
                                "signal_turn_state_change_seq",
                            )?,
                            state_since_ms: optional_number::<u64>(
                                row,
                                "signal_turn_state_since_ms",
                            )?,
                        },
                        session: rows::required(row, "signal_session")?,
                        recorded_ms: number::<u64>(row, "signal_recorded_ms")?,
                        cleared_ms: optional_number::<u64>(row, "signal_cleared_ms")?,
                    })
                } else {
                    None
                },
                signal_history: rows::decoded(row, "signal_history_message")?.unwrap_or_default(),
                signal_inputs: rows::decoded(row, "signal_inputs_message")?.unwrap_or_default(),
                signal_delivered: row.get("signal_delivered")?,
                branch: if row.get::<_, Option<bool>>("branch_present")? == Some(true) {
                    Some(crate::done::Branch {
                        repo: rows::required(row, "branch_repo")?,
                        name: rows::required(row, "branch_name")?,
                        ahead: number::<u64>(row, "branch_ahead")?,
                        landed: row.get("branch_landed")?,
                    })
                } else {
                    None
                },
                detached: row.get("detached")?,
                reply: if row.get::<_, Option<bool>>("reply_present")? == Some(true) {
                    Some(crate::items::Reply {
                        seq: number::<u64>(row, "reply_seq")?,
                        result_id: rows::text(row, "reply_result_id")?,
                        text: rows::required(row, "reply_text")?,
                        captured_ms: number::<u64>(row, "reply_captured_ms")?,
                        reported_ms: optional_number::<u64>(row, "reply_reported_ms")?,
                        done: row.get("reply_done")?,
                    })
                } else {
                    None
                },
                native_done: row.get("native_done")?,
                closed_ms: optional_number::<u64>(row, "closed_ms")?,
                dropped_ms: optional_number::<u64>(row, "dropped_ms")?,
            },
        ));
    }
    Ok(values
        .into_iter()
        .map(|(position, _, mark)| (position, crate::done::key(&mark), mark))
        .collect())
}

pub(super) fn put_hold(
    tx: &Transaction<'_>,
    owner: &str,
    position: i64,
    hold: &crate::holds::Hold,
) -> Result<()> {
    let id = format!("{}:{owner}/{position}", owner.len());
    let mut columns = Columns::default();
    columns.text("row_key", Some(&id));
    columns.text("owner", Some(owner));
    columns.put("position", Sql::Integer(position));
    columns.text("host", Some(&hold.host));
    columns.text("resource", Some(&hold.resource));
    columns.text("hold_owner", Some(&hold.owner));
    columns.text("owner_pane", hold.owner_pane.as_deref());
    columns.text("code", Some(&hold.code));
    columns.text("reason", Some(&hold.reason));
    columns.text("next_command", Some(&hold.next_command));
    columns.text("command_host", Some(&hold.command_host));
    columns.text("request_id", hold.request_id.as_deref());
    columns.upsert(tx, "host_peer_holds", "row_key")?;
    tx.execute("DELETE FROM host_peer_hold_argv WHERE owner=?1", [&id])?;
    for (position, arg) in (0_i64..).zip(&hold.command_argv) {
        tx.execute(
            "INSERT INTO host_peer_hold_argv(owner,position,value) VALUES (?1,?2,?3)",
            rusqlite::params![id, position, arg],
        )?;
    }
    Ok(())
}
pub(super) fn holds(tx: &Transaction<'_>, owner: &str) -> Result<Vec<crate::holds::Hold>> {
    let mut statement =
        tx.prepare_cached("SELECT * FROM host_peer_holds WHERE owner=?1 ORDER BY position")?;
    let mut cursor = statement.query([owner])?;
    let mut holds = Vec::new();
    while let Some(row) = cursor.next()? {
        let id: String = rows::required(row, "row_key")?;
        let command_argv = tx
            .prepare_cached(
                "SELECT value FROM host_peer_hold_argv WHERE owner=?1 ORDER BY position",
            )?
            .query_map([id], |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        holds.push(crate::holds::Hold {
            host: rows::required(row, "host")?,
            resource: rows::required(row, "resource")?,
            owner: rows::required(row, "hold_owner")?,
            owner_pane: rows::text(row, "owner_pane")?,
            code: rows::required(row, "code")?,
            reason: rows::required(row, "reason")?,
            next_command: rows::required(row, "next_command")?,
            command_host: rows::required(row, "command_host")?,
            request_id: rows::text(row, "request_id")?,
            command_argv,
        });
    }
    Ok(holds)
}
