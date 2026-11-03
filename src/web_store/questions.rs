use portal_types::questions::{Pending, Rejection, ResponseItem};
use rusqlite::Transaction;
use rusqlite::types::Value as Sql;

use crate::error::{Error, Result};
use crate::rows::{self, Columns, decoded, required, text};
use crate::web::store::Entry;

pub(crate) fn get(tx: &Transaction<'_>, id: &str) -> Result<Option<Entry>> {
    let mut statement = tx.prepare_cached("SELECT * FROM web_questions WHERE id = ?1")?;
    let mut rows = statement.query([id])?;
    let Some(row) = rows.next()? else {
        return Ok(None);
    };
    let state = required(row, "state")?;
    let answers = if state == "answered" {
        let mut statement =
            tx.prepare_cached("SELECT * FROM web_answers WHERE question = ?1 ORDER BY position")?;
        let mut rows = statement.query([id])?;
        let mut answers = Vec::new();
        while let Some(row) = rows.next()? {
            let key = required(row, "id")?;
            answers.push(ResponseItem { id: required(row, "response_id")?,
                value: decoded(row, "value_message")?.ok_or_else(|| Error::Invalid("missing answer".into()))?,
                attachments: if row.get::<_, bool>("has_attachments")? {
                    Some(rows::list(tx, "SELECT path FROM web_answer_attachments WHERE answer = ?1 ORDER BY position", &key, |row| row.get(0))?)
                } else { None },
            });
        }
        Some(answers)
    } else {
        None
    };
    Ok(Some(Entry {
        pending: Pending {
            id: required(row, "id")?,
            pane: required(row, "pane")?,
            session: required(row, "session")?,
            cwd: required(row, "cwd")?,
            created_ms: rows::unsigned(row.get("created_ms")?)?,
            questions: decoded(row, "form_message")?
                .ok_or_else(|| Error::Invalid("missing form".into()))?,
        },
        answers,
        rejection: text(row, "rejection_reason")?.map(|reason| Rejection { reason }),
        answered_ms: rows::number(row, "answered_ms")?,
    }))
}

pub(crate) fn put(tx: &Transaction<'_>, entry: &Entry) -> Result<()> {
    let pending = &entry.pending;
    if !crate::web::store::valid_id(&pending.id)
        || (entry.answers.is_some() && entry.rejection.is_some())
    {
        return Err(Error::Invalid("invalid question record".into()));
    }
    let state = if entry.rejection.is_some() {
        "rejected"
    } else if entry.answers.is_some() {
        "answered"
    } else {
        "pending"
    };
    let mut columns = Columns::default();
    columns.text("id", Some(&pending.id));
    columns.text("pane", Some(&pending.pane));
    columns.text("session", Some(&pending.session));
    columns.text("cwd", Some(&pending.cwd));
    columns.number("created_ms", Some(pending.created_ms))?;
    columns.text("state", Some(state));
    columns.number("answered_ms", entry.answered_ms)?;
    columns.text(
        "rejection_reason",
        entry.rejection.as_ref().map(|r| r.reason.as_str()),
    );
    columns.message("form_message", Some(&pending.questions))?;
    columns.upsert(tx, "web_questions", "id")?;
    tx.execute("DELETE FROM web_answers WHERE question = ?1", [&pending.id])?;
    for (position, answer) in entry.answers.iter().flatten().enumerate() {
        let id = format!("{}:{position}", pending.id);
        let mut columns = Columns::default();
        columns.text("id", Some(&id));
        columns.text("question", Some(&pending.id));
        columns.number("position", Some(position as u64))?;
        columns.text("response_id", Some(&answer.id));
        columns.message("value_message", Some(&answer.value))?;
        columns.flag("has_attachments", answer.attachments.is_some());
        columns.upsert(tx, "web_answers", "id")?;
        rows::replace_list(
            tx,
            "web_answer_attachments",
            "answer",
            "path",
            &id,
            answer.attachments.iter().flatten().cloned().map(Sql::Text),
        )?;
    }
    Ok(())
}

pub(crate) fn remove(tx: &Transaction<'_>, id: &str) -> Result<bool> {
    Ok(tx.execute("DELETE FROM web_questions WHERE id = ?1", [id])? != 0)
}

pub(crate) fn pending(tx: &Transaction<'_>) -> Result<Vec<Pending>> {
    let ids = rows::list(
        tx,
        "SELECT id FROM web_questions WHERE state = ?1 ORDER BY created_ms, id",
        "pending",
        |row| row.get::<_, String>(0),
    )?;
    ids.into_iter()
        .map(|id| {
            get(tx, &id)?
                .map(|entry| entry.pending)
                .ok_or_else(|| Error::Invalid("missing question".into()))
        })
        .collect()
}
