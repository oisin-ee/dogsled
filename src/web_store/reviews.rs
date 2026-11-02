use portal_types::reviews::{Annotation, Decision, Review, Revision};
use rusqlite::Transaction;
use rusqlite::types::Value as Sql;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::rows::{self, Columns, required, text};
use crate::web::reviews::Record;

pub(crate) fn get(tx: &Transaction<'_>, id: &str) -> Result<Option<Record>> {
    let mut statement = tx.prepare_cached("SELECT * FROM web_reviews WHERE id = ?1")?;
    let mut rows = statement.query([id])?;
    let Some(row) = rows.next()? else {
        return Ok(None);
    };
    let mut revisions = Vec::new();
    let mut statement = tx
        .prepare_cached("SELECT * FROM web_review_revisions WHERE review = ?1 ORDER BY position")?;
    let mut revision_rows = statement.query([id])?;
    while let Some(row) = revision_rows.next()? {
        revisions.push(Revision {
            n: row.get("n")?,
            content: required(row, "content")?,
            submitted_ms: rows::unsigned(row.get("submitted_ms")?)?,
        });
    }
    let mut annotations = Vec::new();
    let mut statement = tx.prepare_cached(
        "SELECT * FROM web_review_annotations WHERE review = ?1 ORDER BY position",
    )?;
    let mut annotation_rows = statement.query([id])?;
    while let Some(row) = annotation_rows.next()? {
        let key = required(row, "id")?;
        annotations.push(Annotation {
            id: required(row, "annotation_id")?,
            revision: row.get("revision")?,
            start_line: super::index(row, "start_line")?,
            end_line: super::index(row, "end_line")?,
            quote: required(row, "quote")?,
            comment: required(row, "comment")?,
            images: rows::list(
                tx,
                "SELECT path FROM web_annotation_images WHERE annotation = ?1 ORDER BY position",
                &key,
                |row| row.get(0),
            )?,
        });
    }
    let decision = match text(row, "decision")?.as_deref() {
        Some("approved") => Some(Decision::Approved {
            notes: text(row, "notes")?,
        }),
        Some("changes_requested") => Some(Decision::ChangesRequested {
            message: required(row, "message")?,
        }),
        None => None,
        _ => return Err(Error::Invalid("invalid review decision".into())),
    };
    Ok(Some(Record {
        review: Review {
            id: required(row, "id")?,
            pane: required(row, "pane")?,
            session: required(row, "session")?,
            cwd: required(row, "cwd")?,
            path: required(row, "path")?,
            general: required(row, "general")?,
            revisions,
            annotations,
            decision,
        },
        delivered: row.get("delivered")?,
    }))
}

pub(crate) fn put(tx: &Transaction<'_>, record: &Record) -> Result<()> {
    let review = &record.review;
    if !crate::web::reviews::valid_id(&review.id) || review.revisions.is_empty() {
        return Err(Error::Invalid("invalid review".into()));
    }
    let mut columns = Columns::default();
    columns.text("id", Some(&review.id));
    columns.text("pane", Some(&review.pane));
    columns.text("session", Some(&review.session));
    columns.text("cwd", Some(&review.cwd));
    columns.text("path", Some(&review.path));
    columns.text("general", Some(&review.general));
    columns.flag("delivered", record.delivered);
    columns.number("latest_ms", review.revisions.last().map(|r| r.submitted_ms))?;
    match &review.decision {
        Some(Decision::Approved { notes }) => {
            columns.text("decision", Some("approved"));
            columns.text("notes", notes.as_deref());
        }
        Some(Decision::ChangesRequested { message }) => {
            columns.text("decision", Some("changes_requested"));
            columns.text("message", Some(message));
        }
        None => (),
    }
    columns.upsert(tx, "web_reviews", "id")?;
    tx.execute(
        "DELETE FROM web_review_revisions WHERE review = ?1",
        [&review.id],
    )?;
    for (position, revision) in review.revisions.iter().enumerate() {
        tx.execute("INSERT INTO web_review_revisions(review, position, n, content, submitted_ms) VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![review.id, rows::integer(position as u64)?, revision.n, revision.content, rows::integer(revision.submitted_ms)?])?;
    }
    tx.execute(
        "DELETE FROM web_review_annotations WHERE review = ?1",
        [&review.id],
    )?;
    for (position, annotation) in review.annotations.iter().enumerate() {
        let id = format!("{}:{position}", review.id);
        let mut columns = Columns::default();
        columns.text("id", Some(&id));
        columns.text("review", Some(&review.id));
        columns.number("position", Some(position as u64))?;
        columns.text("annotation_id", Some(&annotation.id));
        columns.number("revision", Some(u64::from(annotation.revision)))?;
        columns.number("start_line", Some(annotation.start_line as u64))?;
        columns.number("end_line", Some(annotation.end_line as u64))?;
        columns.text("quote", Some(&annotation.quote));
        columns.text("comment", Some(&annotation.comment));
        columns.upsert(tx, "web_review_annotations", "id")?;
        rows::replace_list(
            tx,
            "web_annotation_images",
            "annotation",
            "path",
            &id,
            annotation.images.iter().cloned().map(Sql::Text),
        )?;
    }
    Ok(())
}

pub(crate) fn remove(tx: &Transaction<'_>, id: &str) -> Result<()> {
    tx.execute("DELETE FROM web_reviews WHERE id = ?1", [id])?;
    Ok(())
}

pub(crate) fn select(
    tx: &Transaction<'_>,
    predicate: &str,
    parameters: &[&str],
) -> Result<Vec<Record>> {
    let mut statement = tx.prepare_cached(&format!(
        "SELECT id FROM web_reviews WHERE {predicate} ORDER BY latest_ms DESC, id"
    ))?;
    let ids = statement
        .query_map(rusqlite::params_from_iter(parameters), |row| {
            row.get::<_, String>(0)
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    ids.into_iter()
        .map(|id| get(tx, &id)?.ok_or_else(|| Error::Invalid("missing review".into())))
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Attachment {
    pub name: String,
    pub path: String,
    pub media_type: String,
    pub size: u64,
    pub created_ms: u64,
}

pub(crate) fn attachment(tx: &Transaction<'_>, name: &str) -> Result<Option<Attachment>> {
    let mut statement = tx.prepare_cached("SELECT * FROM web_attachments WHERE name = ?1")?;
    let mut rows = statement.query([name])?;
    rows.next()?
        .map(|row| {
            Ok(Attachment {
                name: required(row, "name")?,
                path: required(row, "path")?,
                media_type: required(row, "media_type")?,
                size: rows::unsigned(row.get("size")?)?,
                created_ms: rows::unsigned(row.get("created_ms")?)?,
            })
        })
        .transpose()
}

pub(crate) fn put_attachment(tx: &Transaction<'_>, attachment: &Attachment) -> Result<()> {
    let mut columns = Columns::default();
    columns.text("name", Some(&attachment.name));
    columns.text("path", Some(&attachment.path));
    columns.text("media_type", Some(&attachment.media_type));
    columns.number("size", Some(attachment.size))?;
    columns.number("created_ms", Some(attachment.created_ms))?;
    columns.upsert(tx, "web_attachments", "name")
}

pub(crate) fn remove_attachment(tx: &Transaction<'_>, name: &str) -> Result<()> {
    tx.execute("DELETE FROM web_attachments WHERE name = ?1", [name])?;
    Ok(())
}
