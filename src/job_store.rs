//! Typed host-owned jobs, request authority, and capacity reservations.

use rusqlite::types::Value as Sql;
use rusqlite::{OptionalExtension, Row, Transaction};

use crate::error::{Error, Result};
use crate::jobs::{Clock, ExecutionScope, Job, Startup, Validity};
use crate::ledger::{Ledger, Reservation};
use crate::rows::{self, Columns, diagnostic_sql, list, number, request_sql, required, text};
use crate::store;

pub(crate) const SCHEMA: &str = concat!(
    "
CREATE TABLE jobs (
    id TEXT PRIMARY KEY CHECK (length(id) > 0 AND id NOT GLOB '*[^A-Za-z0-9-]*'),
    cwd TEXT NOT NULL,
    host TEXT NOT NULL,
    owner TEXT NOT NULL,
    pane TEXT NOT NULL,
    state TEXT NOT NULL,
    exit INTEGER,
    signal INTEGER,
    reason TEXT,
    started_ms INTEGER CHECK (started_ms >= 0),
    ended_ms INTEGER CHECK (ended_ms >= 0),
    timeout_ms INTEGER CHECK (timeout_ms >= 0),
    idle_timeout_ms INTEGER CHECK (idle_timeout_ms >= 0),
    validity_version INTEGER CHECK (validity_version BETWEEN 0 AND 4294967295),
    validity_created_ms INTEGER CHECK (validity_created_ms >= 0),
    startup_version INTEGER CHECK (startup_version BETWEEN 0 AND 4294967295),
    startup_scope TEXT CHECK (startup_scope IN ('git', 'non_repository', 'unsupported')),
    startup_cwd TEXT,
    binding_message BLOB,
    checkout_repo TEXT,
    checkout_path TEXT,
    checkout_branch TEXT,
    checkout_head TEXT,
    checkout_common TEXT,
    checkout_gitdir TEXT,
    checkout_generation TEXT,
    output_expired INTEGER NOT NULL CHECK (output_expired IN (0, 1)),
    CHECK ((validity_version IS NULL) = (validity_created_ms IS NULL)),
    CHECK ((startup_version IS NULL) = (startup_scope IS NULL)),
    CHECK ((checkout_common IS NULL) = (checkout_generation IS NULL))
) STRICT;
CREATE INDEX jobs_expiry ON jobs (ended_ms) WHERE state IN ('idle', 'blocked');
CREATE TABLE job_stops (
    job TEXT PRIMARY KEY REFERENCES jobs (id) ON DELETE CASCADE
) STRICT;
CREATE TABLE job_args (
    job TEXT NOT NULL REFERENCES jobs (id) ON DELETE CASCADE,
    position INTEGER NOT NULL CHECK (position >= 0),
    arg TEXT NOT NULL,
    PRIMARY KEY (job, position)
) STRICT, WITHOUT ROWID;
CREATE TABLE job_validity (
    id TEXT PRIMARY KEY CHECK (length(id) > 0 AND id NOT GLOB '*[^A-Za-z0-9-]*'),
    version INTEGER NOT NULL CHECK (version BETWEEN 0 AND 4294967295),
    created_ms INTEGER NOT NULL CHECK (created_ms >= 0),
    owner TEXT CHECK (owner IS NULL OR length(owner) > 0),
    owner_host TEXT,
    unstarted_finished_ms INTEGER CHECK (unstarted_finished_ms >= 0)
) STRICT;
CREATE INDEX job_validity_created ON job_validity (created_ms);
CREATE TABLE request_ingress (
    id TEXT PRIMARY KEY CHECK (length(id) > 0 AND id NOT GLOB '*[^A-Za-z0-9-]*'),
    version INTEGER NOT NULL CHECK (version = 1)
) STRICT;
CREATE TABLE job_clock (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    version INTEGER NOT NULL CHECK (version BETWEEN 0 AND 4294967295),
    high_water_ms INTEGER NOT NULL CHECK (high_water_ms >= 0)
) STRICT;
CREATE TABLE placement_attempts (
    id TEXT PRIMARY KEY CHECK (length(id) > 0 AND id NOT GLOB '*[^A-Za-z0-9-]*'),
    host TEXT NOT NULL
) STRICT;
CREATE TABLE placements (
    id TEXT PRIMARY KEY CHECK (length(id) > 0 AND id NOT GLOB '*[^A-Za-z0-9-]*'),
    position INTEGER NOT NULL CHECK (position >= 0),
    created_ms INTEGER NOT NULL CHECK (created_ms >= 0)
) STRICT;
CREATE INDEX placements_position ON placements (position);
CREATE INDEX placements_created ON placements (created_ms);
CREATE TABLE cancellations (
    id TEXT PRIMARY KEY CHECK (length(id) > 0 AND id NOT GLOB '*[^A-Za-z0-9-]*'),
    version INTEGER NOT NULL CHECK (version BETWEEN 0 AND 4294967295),
    completion_version INTEGER CHECK (completion_version BETWEEN 0 AND 4294967295),
    finished_ms INTEGER CHECK (finished_ms >= 0),
    operation_id TEXT NOT NULL,
    ticket_id TEXT NOT NULL,
    ticket_created_ms INTEGER NOT NULL CHECK (ticket_created_ms >= 0),
    ticket_finished_ms INTEGER CHECK (ticket_finished_ms >= 0),
    ticket_state TEXT NOT NULL,
    ticket_waiting INTEGER NOT NULL CHECK (ticket_waiting IN (0, 1)),
    ticket_waiting_until_ms INTEGER CHECK (ticket_waiting_until_ms >= 0),
    ticket_wait_reason TEXT,
    ticket_notified INTEGER NOT NULL CHECK (ticket_notified IN (0, 1)),
    ticket_result_message BLOB,
    ticket_error TEXT,
    retry_attempts INTEGER NOT NULL CHECK (retry_attempts BETWEEN 0 AND 4294967295),
    retry_next_ms INTEGER NOT NULL CHECK (retry_next_ms >= 0),",
    request_sql!(),
    diagnostic_sql!(),
    "
    attempt_message BLOB,
    preparation_message BLOB,
    placement_message BLOB,
    intent_message BLOB,
    acknowledgement_message BLOB,
    outcome TEXT NOT NULL CHECK (outcome IN ('possibly_executed', 'not_started', 'unknown')),
    release TEXT NOT NULL,
    error TEXT,
    held_reason TEXT,
    held_evidence TEXT,
    updated_seq INTEGER NOT NULL,
    CHECK ((held_reason IS NULL) = (held_evidence IS NULL)),
    CHECK (request_id = id)
) STRICT;
CREATE INDEX cancellations_repair ON cancellations (updated_seq, id);
CREATE INDEX cancellations_pending ON cancellations (release, updated_seq);
CREATE TABLE cancellation_job_args (
    cancellation TEXT NOT NULL REFERENCES cancellations (id) ON DELETE CASCADE,
    position INTEGER NOT NULL CHECK (position >= 0),
    arg TEXT NOT NULL,
    PRIMARY KEY (cancellation, position)
) STRICT, WITHOUT ROWID;
"
);

pub(crate) fn get(ledger: &Ledger, id: &str) -> Result<Option<Job>> {
    store::read(ledger, |tx| load(tx, id))
}

pub(crate) fn put_named(ledger: &Ledger, id: &str, job: &Job) -> Result<()> {
    if job.id != id {
        return Err(Error::Invalid("job identity changed".into()));
    }
    put(ledger, job)
}

pub(crate) fn put(ledger: &Ledger, job: &Job) -> Result<()> {
    store::write(ledger, |tx| save(tx, job))
}

pub(crate) fn remove(ledger: &Ledger, id: &str) -> Result<()> {
    store::write(ledger, |tx| {
        tx.execute("DELETE FROM jobs WHERE id = ?1", [id])?;
        Ok(())
    })
}

pub(crate) fn expiry_ids(ledger: &Ledger, now: u64) -> Result<Vec<String>> {
    store::read(ledger, |tx| {
        let mut statement = tx.prepare_cached("SELECT id FROM jobs WHERE state IN ('idle', 'blocked') AND ended_ms < ?1 ORDER BY ended_ms, id")?;
        let ids = statement.query_map(
            [rows::integer(now.saturating_sub(crate::jobs::LOG_KEEP_MS))?],
            |row| row.get(0),
        )?;
        Ok(ids.collect::<rusqlite::Result<_>>()?)
    })
}

pub(crate) fn all(ledger: &Ledger) -> Result<Vec<Job>> {
    store::read(ledger, |tx| {
        let mut statement = tx.prepare_cached("SELECT * FROM jobs ORDER BY id")?;
        let mut rows = statement.query([])?;
        let mut jobs = Vec::new();
        while let Some(row) = rows.next()? {
            jobs.push(job_row(tx, row)?);
        }
        Ok(jobs)
    })
}

pub(crate) fn load(tx: &Transaction<'_>, id: &str) -> Result<Option<Job>> {
    let mut statement = tx.prepare_cached("SELECT * FROM jobs WHERE id = ?1")?;
    let mut rows = statement.query([id])?;
    rows.next()?.map(|row| job_row(tx, row)).transpose()
}

pub(crate) fn save(tx: &Transaction<'_>, job: &Job) -> Result<()> {
    crate::jobs::validate_id(&job.id)?;
    let mut columns = Columns::default();
    columns.text("id", Some(&job.id));
    columns.text("cwd", Some(&job.cwd));
    columns.text("host", Some(&job.host));
    columns.text("owner", Some(&job.owner));
    columns.text("pane", Some(&job.pane));
    columns.text("state", Some(&job.state));
    columns.put(
        "exit",
        job.exit.map_or(Sql::Null, |v| Sql::Integer(i64::from(v))),
    );
    columns.put(
        "signal",
        job.signal.map_or(Sql::Null, |v| Sql::Integer(i64::from(v))),
    );
    columns.text("reason", job.reason.as_deref());
    columns.number("started_ms", job.started_ms)?;
    columns.number("ended_ms", job.ended_ms)?;
    columns.number("timeout_ms", job.timeout_ms)?;
    columns.number("idle_timeout_ms", job.idle_timeout_ms)?;
    columns.number(
        "validity_version",
        job.validity.map(|v| u64::from(v.version)),
    )?;
    columns.number("validity_created_ms", job.validity.map(|v| v.created_ms))?;
    if let Some(startup) = &job.startup {
        columns.number("startup_version", Some(u64::from(startup.version)))?;
        columns.message("binding_message", startup.preparation.as_ref())?;
        match &startup.scope {
            ExecutionScope::Git { checkout } => {
                columns.text("startup_scope", Some("git"));
                columns.text("checkout_repo", Some(&checkout.repo));
                columns.text("checkout_path", Some(&checkout.path));
                columns.text("checkout_branch", Some(&checkout.branch));
                columns.text("checkout_head", Some(&checkout.head));
                if let Some(identity) = &checkout.identity {
                    columns.text("checkout_common", Some(path(&identity.common)?));
                    columns.text("checkout_gitdir", Some(path(&identity.gitdir)?));
                    columns.text("checkout_generation", Some(&identity.generation));
                }
            }
            ExecutionScope::NonRepository { cwd } => {
                columns.text("startup_scope", Some("non_repository"));
                columns.text("startup_cwd", Some(cwd));
            }
            ExecutionScope::Unsupported => columns.text("startup_scope", Some("unsupported")),
        }
    }
    columns.flag("output_expired", job.output_expired);
    columns.upsert(tx, "jobs", "id")?;
    rows::replace_list(
        tx,
        "job_args",
        "job",
        "arg",
        &job.id,
        job.cmd.iter().cloned().map(Sql::Text),
    )?;
    Ok(())
}

fn path(path: &std::path::Path) -> Result<&str> {
    path.to_str()
        .ok_or_else(|| Error::Invalid("state path is not UTF-8".into()))
}

fn job_row(tx: &Transaction<'_>, row: &Row<'_>) -> Result<Job> {
    let id = required(row, "id")?;
    let startup = match number(row, "startup_version")? {
        Some(version) => Some(Startup {
            version: u32::try_from(version)
                .map_err(|_| Error::Invalid("startup version".into()))?,
            preparation: rows::decoded(row, "binding_message")?,
            scope: match required(row, "startup_scope")?.as_str() {
                "git" => ExecutionScope::Git {
                    checkout: crate::cleanup::Tree {
                        repo: required(row, "checkout_repo")?,
                        path: required(row, "checkout_path")?,
                        branch: required(row, "checkout_branch")?,
                        head: required(row, "checkout_head")?,
                        identity: text(row, "checkout_common")?
                            .map(|common| {
                                Ok::<_, Error>(crate::worktree::Identity {
                                    common: common.into(),
                                    gitdir: required(row, "checkout_gitdir")?.into(),
                                    generation: required(row, "checkout_generation")?,
                                })
                            })
                            .transpose()?,
                    },
                },
                "non_repository" => ExecutionScope::NonRepository {
                    cwd: required(row, "startup_cwd")?,
                },
                _ => ExecutionScope::Unsupported,
            },
        }),
        None => None,
    };
    Ok(Job {
        cmd: list(
            tx,
            "SELECT arg FROM job_args WHERE job = ?1 ORDER BY position",
            &id,
            |row| row.get(0),
        )?,
        id,
        cwd: required(row, "cwd")?,
        host: required(row, "host")?,
        owner: required(row, "owner")?,
        pane: required(row, "pane")?,
        state: required(row, "state")?,
        exit: row.get("exit")?,
        signal: row.get("signal")?,
        reason: text(row, "reason")?,
        started_ms: number(row, "started_ms")?,
        ended_ms: number(row, "ended_ms")?,
        timeout_ms: number(row, "timeout_ms")?,
        idle_timeout_ms: number(row, "idle_timeout_ms")?,
        validity: rows::validity(
            row.get("validity_version")?,
            row.get("validity_created_ms")?,
        )?,
        startup,
        output_expired: number(row, "output_expired")? == Some(1),
    })
}

pub(crate) fn stop_requested(ledger: &Ledger, id: &str) -> Result<bool> {
    store::read(ledger, |tx| {
        Ok(tx
            .prepare_cached("SELECT 1 FROM job_stops WHERE job = ?1")?
            .exists([id])?)
    })
}

pub(crate) fn request_stop(ledger: &Ledger, id: &str, requested: bool) -> Result<()> {
    store::write(ledger, |tx| {
        if requested {
            tx.execute("INSERT OR IGNORE INTO job_stops (job) VALUES (?1)", [id])?;
        } else {
            tx.execute("DELETE FROM job_stops WHERE job = ?1", [id])?;
        }
        Ok(())
    })
}

/// Immutable custody provenance survives independent retention of request artifacts.
pub(crate) fn remember_owner(
    ledger: &Ledger,
    id: &str,
    owner: &str,
    origin_host: Option<&str>,
) -> Result<()> {
    store::write(ledger, |tx| remember_owner_in(tx, id, owner, origin_host))
}

pub(crate) fn remember_owner_in(
    tx: &Transaction<'_>,
    id: &str,
    owner: &str,
    origin_host: Option<&str>,
) -> Result<()> {
    crate::jobs::validate_id(id)?;
    let existing: Option<(String, Option<String>)> = tx
        .query_row(
            "SELECT owner, owner_host FROM job_validity WHERE id = ?1 AND owner IS NOT NULL",
            [id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    if existing.as_ref().is_some_and(|(saved, origin)| {
        saved != owner
            || origin
                .as_deref()
                .zip(origin_host)
                .is_some_and(|(a, b)| a != b)
    }) {
        return Err(crate::error::Error::Invalid("request owner changed".into()));
    }
    tx.execute(
        "UPDATE job_validity SET owner = ?2, owner_host = COALESCE(owner_host, ?3) WHERE id = ?1",
        rusqlite::params![id, owner, origin_host],
    )?;
    Ok(())
}

pub(crate) struct RequestOwner {
    pub id: String,
    pub owner: String,
    pub owner_host: Option<String>,
}

pub(crate) fn owners(ledger: &Ledger) -> Result<Vec<RequestOwner>> {
    store::read(ledger, |tx| {
        let mut statement = tx.prepare_cached(
            "SELECT id, owner, owner_host FROM job_validity WHERE owner IS NOT NULL ORDER BY id",
        )?;
        Ok(statement
            .query_map([], |row| {
                Ok(RequestOwner {
                    id: row.get(0)?,
                    owner: row.get(1)?,
                    owner_host: row.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?)
    })
}

pub(crate) fn owner(ledger: &Ledger, id: &str) -> Result<(String, String)> {
    let owner = owners(ledger)?
        .into_iter()
        .find(|owner| owner.id == id)
        .ok_or_else(|| {
            crate::error::Error::Invalid("reservation request has no recorded owner".into())
        })?;
    let host = owner.owner_host.ok_or_else(|| {
        crate::error::Error::Invalid("reservation request has no recorded owner host".into())
    })?;
    Ok((owner.owner, host))
}

pub(crate) fn unstarted_finished(ledger: &Ledger, id: &str) -> Result<Option<u64>> {
    store::read(ledger, |tx| {
        tx.query_row(
            "SELECT unstarted_finished_ms FROM job_validity WHERE id = ?1",
            [id],
            |row| row.get::<_, Option<i64>>(0),
        )
        .optional()?
        .flatten()
        .map(rows::unsigned)
        .transpose()
    })
}

pub(crate) fn finish_unstarted(ledger: &Ledger, id: &str, finished: u64) -> Result<()> {
    store::write(ledger, |tx| {
        tx.execute("UPDATE job_validity SET unstarted_finished_ms = COALESCE(unstarted_finished_ms, ?2) WHERE id = ?1",
            rusqlite::params![id, rows::integer(finished)?])?;
        Ok(())
    })
}

pub(crate) fn validity(tx: &Transaction<'_>, id: &str) -> Result<Option<Validity>> {
    let mut statement =
        tx.prepare_cached("SELECT version, created_ms FROM job_validity WHERE id = ?1")?;
    let mut rows = statement.query([id])?;
    rows.next()?
        .map(|row| rows::validity(row.get(0)?, row.get(1)?))
        .transpose()
        .map(Option::flatten)
}

pub(crate) fn put_validity(tx: &Transaction<'_>, id: &str, validity: Validity) -> Result<()> {
    crate::jobs::validate_id(id)?;
    tx.execute(
        "INSERT INTO job_validity (id, version, created_ms) VALUES (?1, ?2, ?3)
         ON CONFLICT (id) DO UPDATE SET version = excluded.version, created_ms = excluded.created_ms",
        rusqlite::params![id, validity.version, rows::integer(validity.created_ms)?],
    )?;
    Ok(())
}

/// A request's validity with the owner recorded on the same row.
pub(crate) struct OwnedValidity {
    pub id: String,
    pub validity: Validity,
    pub owner: Option<String>,
    pub owner_host: Option<String>,
}

/// Every validity and its owner from one read, so a reader never pairs a validity row with
/// an owner list captured before that row was issued.
pub(crate) fn owned_validities(ledger: &Ledger) -> Result<Vec<OwnedValidity>> {
    store::read(ledger, |tx| {
        let mut statement = tx.prepare_cached(
            "SELECT id, version, created_ms, owner, owner_host FROM job_validity
             ORDER BY created_ms, id",
        )?;
        let mut rows = statement.query([])?;
        let mut values = Vec::new();
        while let Some(row) = rows.next()? {
            values.push(OwnedValidity {
                id: required(row, "id")?,
                validity: Validity {
                    version: row.get("version")?,
                    created_ms: rows::unsigned(row.get("created_ms")?)?,
                },
                owner: row.get("owner")?,
                owner_host: row.get("owner_host")?,
            });
        }
        Ok(values)
    })
}

pub(crate) fn validities(ledger: &Ledger) -> Result<Vec<(String, Validity)>> {
    store::read(ledger, |tx| {
        let mut statement = tx.prepare_cached(
            "SELECT id, version, created_ms FROM job_validity ORDER BY created_ms, id",
        )?;
        let mut rows = statement.query([])?;
        let mut values = Vec::new();
        while let Some(row) = rows.next()? {
            values.push((
                required(row, "id")?,
                Validity {
                    version: row.get("version")?,
                    created_ms: rows::unsigned(row.get("created_ms")?)?,
                },
            ));
        }
        Ok(values)
    })
}

/// The caller has excluded admissions, preparations, tickets, and launch owners.
pub(crate) fn expire_validity_in(tx: &rusqlite::Transaction<'_>, id: &str, now: u64) -> Result<()> {
    let finished: Option<i64> = tx
        .query_row(
            "SELECT unstarted_finished_ms FROM job_validity WHERE id = ?1",
            [id],
            |row| row.get(0),
        )
        .optional()?
        .flatten();
    let finished = finished.map(rows::unsigned).transpose()?;
    if validity(tx, id)?.is_some_and(|validity| {
        now.saturating_sub(finished.unwrap_or(validity.created_ms)) > crate::jobs::RECORD_KEEP_MS
    }) && load(tx, id)?
        .is_none_or(|job| matches!(crate::jobs::expiry(&job, now), crate::jobs::Expiry::Record))
    {
        tx.execute("DELETE FROM job_validity WHERE id = ?1", [id])?;
        tx.execute("DELETE FROM request_ingress WHERE id = ?1", [id])?;
    }
    Ok(())
}

pub(crate) fn ingress(tx: &Transaction<'_>, id: &str) -> Result<bool> {
    Ok(tx
        .prepare_cached("SELECT 1 FROM request_ingress WHERE id = ?1")?
        .exists([id])?)
}

pub(crate) fn put_ingress(tx: &Transaction<'_>, id: &str) -> Result<()> {
    crate::jobs::validate_id(id)?;
    let mut columns = Columns::default();
    columns.text("id", Some(id));
    columns.number("version", Some(1))?;
    columns.upsert(tx, "request_ingress", "id")
}

pub(crate) fn clock(tx: &Transaction<'_>) -> Result<Option<Clock>> {
    let mut statement =
        tx.prepare_cached("SELECT version, high_water_ms FROM job_clock WHERE id = 1")?;
    let mut rows = statement.query([])?;
    rows.next()?
        .map(|row| {
            Ok(Clock {
                version: row.get("version")?,
                high_water_ms: rows::unsigned(row.get("high_water_ms")?)?,
            })
        })
        .transpose()
}

pub(crate) fn put_clock(tx: &Transaction<'_>, clock: &Clock) -> Result<()> {
    let mut columns = Columns::default();
    columns.number("id", Some(1))?;
    columns.number("version", Some(u64::from(clock.version)))?;
    columns.number("high_water_ms", Some(clock.high_water_ms))?;
    columns.upsert(tx, "job_clock", "id")
}

pub(crate) fn selection(ledger: &Ledger, id: &str) -> Result<Option<String>> {
    store::read(ledger, |tx| {
        let mut statement =
            tx.prepare_cached("SELECT host FROM placement_attempts WHERE id = ?1")?;
        let mut rows = statement.query([id])?;
        rows.next()?.map(|row| Ok(row.get(0)?)).transpose()
    })
}

pub(crate) fn put_selection(ledger: &Ledger, id: &str, host: Option<&str>) -> Result<()> {
    crate::jobs::validate_id(id)?;
    store::write(ledger, |tx| {
        if let Some(host) = host {
            let mut columns = Columns::default();
            columns.text("id", Some(id));
            columns.text("host", Some(host));
            columns.upsert(tx, "placement_attempts", "id")?;
        } else {
            tx.execute("DELETE FROM placement_attempts WHERE id = ?1", [id])?;
        }
        Ok(())
    })
}

#[cfg(test)]
pub(crate) fn cancellation_revision(ledger: &Ledger, id: &str) -> Result<i64> {
    store::read(ledger, |tx| {
        Ok(tx.query_row(
            "SELECT updated_seq FROM cancellations WHERE id = ?1",
            [id],
            |row| row.get(0),
        )?)
    })
}

pub(crate) fn cancellation(ledger: &Ledger, id: &str) -> Result<Option<crate::work::Cancellation>> {
    store::read(ledger, |tx| {
        let mut statement = tx.prepare_cached("SELECT * FROM cancellations WHERE id = ?1")?;
        let mut rows = statement.query([id])?;
        rows.next()?
            .map(|row| cancellation_row(tx, row))
            .transpose()
    })
}

pub(crate) fn cancellations(ledger: &Ledger) -> Result<Vec<crate::work::Cancellation>> {
    store::read(ledger, |tx| {
        let mut statement =
            tx.prepare_cached("SELECT * FROM cancellations ORDER BY updated_seq, id")?;
        let mut rows = statement.query([])?;
        let mut records = Vec::new();
        while let Some(row) = rows.next()? {
            records.push(cancellation_row(tx, row)?);
        }
        Ok(records)
    })
}

pub(crate) fn remove_cancellation(ledger: &Ledger, id: &str) -> Result<()> {
    store::write(ledger, |tx| remove_cancellation_in(tx, id))
}

/// [`remove_cancellation`] inside the caller's write transaction.
pub(crate) fn remove_cancellation_in(tx: &rusqlite::Transaction<'_>, id: &str) -> Result<()> {
    tx.execute("DELETE FROM cancellations WHERE id = ?1", [id])?;
    Ok(())
}

pub(crate) fn put_cancellation(ledger: &Ledger, saved: &crate::work::Cancellation) -> Result<()> {
    let ticket = &saved.ticket;
    let id = &ticket.request.id;
    crate::jobs::validate_id(id)?;
    crate::jobs::validate_id(&saved.operation_id)?;
    // These snapshots are exact peer/identity evidence. The request and ticket fields
    // used by local decisions have typed columns; no query inspects a message.
    let mut columns = Columns::default();
    columns.text("id", Some(id));
    columns.number("version", Some(u64::from(saved.version)))?;
    columns.number(
        "completion_version",
        saved.completion_version.map(u64::from),
    )?;
    columns.number("finished_ms", saved.finished_ms)?;
    columns.text("operation_id", Some(&saved.operation_id));
    columns.text("ticket_id", Some(&ticket.id));
    columns.number("ticket_created_ms", Some(ticket.created_ms))?;
    columns.number("ticket_finished_ms", ticket.finished_ms)?;
    columns.text("ticket_state", Some(&ticket.state));
    columns.flag("ticket_waiting", ticket.waiting);
    columns.number("ticket_waiting_until_ms", ticket.waiting_until_ms)?;
    columns.text("ticket_wait_reason", ticket.wait_reason.as_deref());
    columns.flag("ticket_notified", ticket.notified);
    columns.message("ticket_result_message", ticket.result.as_ref())?;
    columns.text("ticket_error", ticket.error.as_deref());
    columns.number("retry_attempts", Some(u64::from(ticket.retry.attempts)))?;
    columns.number("retry_next_ms", Some(ticket.retry.next_ms))?;
    let args = rows::request_columns(&mut columns, &ticket.request)?;
    rows::diagnostic_columns(&mut columns, ticket.diagnostic.as_ref())?;
    columns.message("attempt_message", saved.admission.as_ref())?;
    columns.message("preparation_message", saved.preparation.as_ref())?;
    columns.message("placement_message", saved.placement.as_ref())?;
    columns.message("intent_message", saved.intent.as_ref())?;
    columns.message("acknowledgement_message", saved.acknowledgement.as_ref())?;
    columns.text("outcome", Some(&rows::name(&saved.outcome)?));
    columns.text("release", Some(&saved.release));
    columns.text("error", saved.error.as_deref());
    if let Some(held) = &saved.held {
        columns.text("held_reason", Some(&held.reason));
        columns.text("held_evidence", Some(&held.evidence));
    }
    store::write(ledger, |tx| {
        let sequence: i64 = tx.query_row(
            "SELECT COALESCE(MAX(updated_seq), 0) + 1 FROM cancellations",
            [],
            |row| row.get(0),
        )?;
        columns.put("updated_seq", Sql::Integer(sequence));
        columns.upsert(tx, "cancellations", "id")?;
        rows::replace_list(
            tx,
            "cancellation_job_args",
            "cancellation",
            "arg",
            id,
            args.into_iter().map(Sql::Text),
        )
    })
}

fn cancellation_row(tx: &Transaction<'_>, row: &Row<'_>) -> Result<crate::work::Cancellation> {
    let id = required(row, "id")?;
    let request = rows::load_request(row, || {
        list(
            tx,
            "SELECT arg FROM cancellation_job_args WHERE cancellation = ?1 ORDER BY position",
            &id,
            |row| row.get(0),
        )
    })?;
    Ok(crate::work::Cancellation {
        version: row.get("version")?,
        completion_version: row.get("completion_version")?,
        finished_ms: number(row, "finished_ms")?,
        operation_id: required(row, "operation_id")?,
        ticket: crate::work::Ticket {
            id: required(row, "ticket_id")?,
            finished_ms: number(row, "ticket_finished_ms")?,
            request,
            created_ms: rows::unsigned(row.get("ticket_created_ms")?)?,
            state: required(row, "ticket_state")?,
            waiting: number(row, "ticket_waiting")? == Some(1),
            waiting_until_ms: number(row, "ticket_waiting_until_ms")?,
            wait_reason: text(row, "ticket_wait_reason")?,
            notified: number(row, "ticket_notified")? == Some(1),
            result: rows::decoded(row, "ticket_result_message")?,
            error: text(row, "ticket_error")?,
            diagnostic: rows::load_diagnostic(row)?,
            retry: crate::backoff::Backoff {
                attempts: row.get("retry_attempts")?,
                next_ms: rows::unsigned(row.get("retry_next_ms")?)?,
            },
        },
        admission: rows::decoded(row, "attempt_message")?,
        preparation: rows::decoded(row, "preparation_message")?,
        placement: rows::decoded(row, "placement_message")?,
        outcome: rows::variant(required(row, "outcome")?)?,
        release: required(row, "release")?,
        intent: rows::decoded(row, "intent_message")?,
        acknowledgement: rows::decoded(row, "acknowledgement_message")?,
        error: text(row, "error")?,
        held: text(row, "held_reason")?
            .map(|reason| {
                Ok::<_, Error>(crate::work::Held {
                    reason,
                    evidence: required(row, "held_evidence")?,
                })
            })
            .transpose()?,
    })
}

pub(crate) fn live_reservations(ledger: &Ledger, now: u64, ttl: u64) -> Result<Vec<Reservation>> {
    if ttl == 0 {
        return Ok(Vec::new());
    }
    let oldest = if now < ttl {
        -1
    } else {
        rows::integer(now - ttl)?
    };
    store::read(ledger, |tx| {
        let mut statement = tx.prepare_cached(
            "SELECT id, created_ms FROM placements WHERE created_ms > ?1 ORDER BY position",
        )?;
        let mut rows = statement.query([oldest])?;
        let mut values = Vec::new();
        while let Some(row) = rows.next()? {
            values.push(Reservation {
                id: required(row, "id")?,
                created_ms: rows::unsigned(row.get("created_ms")?)?,
            });
        }
        Ok(values)
    })
}

pub(crate) fn reservations(ledger: &Ledger) -> Result<Vec<Reservation>> {
    store::read(ledger, |tx| {
        let mut statement =
            tx.prepare_cached("SELECT id, created_ms FROM placements ORDER BY position")?;
        let mut rows = statement.query([])?;
        let mut values = Vec::new();
        while let Some(row) = rows.next()? {
            values.push(Reservation {
                id: required(row, "id")?,
                created_ms: rows::unsigned(row.get("created_ms")?)?,
            });
        }
        Ok(values)
    })
}

pub(crate) fn release_reservation(ledger: &Ledger, id: &str) -> Result<()> {
    ledger.with_lock("host", || {
        store::write(ledger, |tx| {
            tx.execute("DELETE FROM placements WHERE id = ?1", [id])?;
            Ok(())
        })
    })
}

pub(crate) fn put_reservations(ledger: &Ledger, reservations: &[Reservation]) -> Result<()> {
    store::write(ledger, |tx| {
        tx.execute("DELETE FROM placements", [])?;
        let mut ids = std::collections::HashSet::new();
        for (position, reservation) in reservations.iter().enumerate() {
            crate::jobs::validate_id(&reservation.id)?;
            if !ids.insert(&reservation.id) {
                return Err(Error::Invalid("duplicate reservation id".into()));
            }
            let mut columns = Columns::default();
            columns.text("id", Some(&reservation.id));
            columns.number("position", Some(position as u64))?;
            columns.number("created_ms", Some(reservation.created_ms))?;
            columns.upsert(tx, "placements", "id")?;
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ledger() -> Ledger {
        Ledger::new(std::env::temp_dir().join(crate::ledger::new_id()))
    }

    fn job() -> Job {
        serde_json::from_value(json!({
            "id":"roundtrip", "cmd":["sh", "-c", "printf 'a\\nb'"], "cwd":"/checkout", "host":"local",
            "owner":"owner", "pane":"pane", "state":"blocked", "exit":17, "signal":9, "reason":"reason",
            "started_ms":101, "ended_ms":202, "timeout_ms":303, "idle_timeout_ms":404,
            "validity":{"version":1,"created_ms":99}, "output_expired":true,
            "startup":{"version":1,"scope":"git", "checkout":{
                "repo":"/repo", "path":"/checkout", "branch":"feat/job", "head":"abc",
                "identity":{"common":"/repo/.git", "gitdir":"/repo/.git/worktrees/job", "generation":"generation"}
            }, "preparation":{
                "request_id":"roundtrip", "request":["/repo","feat/job","main"],
                "claim":{"request_id":"roundtrip", "tree":{"repo":"/repo","path":"/checkout","branch":"feat/job","head":"abc"}},
                "prepared":{"pane_id":"pane","terminal_id":"terminal","workspace_id":"workspace","path":"/checkout"}
            }}
        })).unwrap()
    }

    #[test]
    fn jobs_and_arguments_roundtrip_clear_children_and_reject_invalid_input() {
        let ledger = ledger();
        let mut job = job();
        put(&ledger, &job).unwrap();
        assert_eq!(get(&ledger, &job.id).unwrap(), Some(job.clone()));
        assert_eq!(all(&ledger).unwrap(), [job.clone()]);
        job.cmd.clear();
        job.startup = Some(Startup {
            version: 1,
            preparation: None,
            scope: ExecutionScope::NonRepository { cwd: "/tmp".into() },
        });
        job.validity = None;
        job.exit = None;
        job.signal = None;
        job.reason = None;
        put(&ledger, &job).unwrap();
        assert_eq!(get(&ledger, &job.id).unwrap(), Some(job.clone()));
        job.startup = Some(Startup {
            version: 99,
            preparation: None,
            scope: ExecutionScope::Unsupported,
        });
        put(&ledger, &job).unwrap();
        assert_eq!(get(&ledger, &job.id).unwrap(), Some(job.clone()));
        job.startup = None;
        job.cmd = vec!["replacement".into()];
        put(&ledger, &job).unwrap();
        assert_eq!(get(&ledger, &job.id).unwrap(), Some(job.clone()));
        let mut invalid = job.clone();
        invalid.id = "bad/id".into();
        assert!(put(&ledger, &invalid).is_err());
        assert!(get(&ledger, &invalid.id).unwrap().is_none());
        invalid.id = "overflow".into();
        invalid.ended_ms = Some(u64::MAX);
        assert!(put(&ledger, &invalid).is_err());
        assert!(get(&ledger, "overflow").unwrap().is_none());
        assert!(put_named(&ledger, "wrong", &job).is_err());
        assert!(get(&ledger, "wrong").unwrap().is_none());
        remove(&ledger, &job.id).unwrap();
        assert_eq!(all(&ledger).unwrap(), [] as [Job; 0]);
        std::fs::remove_dir_all(ledger.dir).unwrap();
    }

    #[test]
    fn stop_requests_roundtrip_and_follow_job_lifetime() {
        let ledger = ledger();
        let job = job();
        put(&ledger, &job).unwrap();
        request_stop(&ledger, &job.id, true).unwrap();
        assert!(stop_requested(&ledger, &job.id).unwrap());
        request_stop(&ledger, &job.id, false).unwrap();
        assert!(!stop_requested(&ledger, &job.id).unwrap());
        assert!(request_stop(&ledger, "absent", true).is_err());
        assert!(!stop_requested(&ledger, "absent").unwrap());
        request_stop(&ledger, &job.id, true).unwrap();
        remove(&ledger, &job.id).unwrap();
        assert!(!stop_requested(&ledger, &job.id).unwrap());
        std::fs::remove_dir_all(ledger.dir).unwrap();
    }

    #[test]
    fn validity_roundtrip_and_invalid_write_rolls_back() {
        let ledger = ledger();
        let stamp = Validity::new(101);
        store::write(&ledger, |tx| put_validity(tx, "request", stamp)).unwrap();
        assert_eq!(
            store::read(&ledger, |tx| validity(tx, "request")).unwrap(),
            Some(stamp)
        );
        assert_eq!(validities(&ledger).unwrap(), [("request".into(), stamp)]);
        store::write(&ledger, |tx| {
            put_validity(tx, "request", Validity::new(202))
        })
        .unwrap();
        assert_eq!(validities(&ledger).unwrap()[0].1.created_ms, 202);
        assert!(
            store::write(&ledger, |tx| put_validity(
                tx,
                "invalid",
                Validity::new(u64::MAX)
            ))
            .is_err()
        );
        assert!(
            store::read(&ledger, |tx| validity(tx, "invalid"))
                .unwrap()
                .is_none()
        );
        store::write(&ledger, |tx| {
            expire_validity_in(tx, "request", 203 + crate::jobs::RECORD_KEEP_MS)
        })
        .unwrap();
        assert_eq!(validities(&ledger).unwrap(), [] as [(String, Validity); 0]);
        std::fs::remove_dir_all(ledger.dir).unwrap();
    }

    #[test]
    fn ingress_roundtrip_and_invalid_id_is_not_stored() {
        let ledger = ledger();
        store::write(&ledger, |tx| put_ingress(tx, "request")).unwrap();
        assert!(store::read(&ledger, |tx| ingress(tx, "request")).unwrap());
        store::write(&ledger, |tx| put_ingress(tx, "request")).unwrap();
        assert!(store::write(&ledger, |tx| put_ingress(tx, "bad/id")).is_err());
        assert!(!store::read(&ledger, |tx| ingress(tx, "bad/id")).unwrap());
        std::fs::remove_dir_all(ledger.dir).unwrap();
    }

    #[test]
    fn clock_roundtrip_and_invalid_time_is_not_stored() {
        let ledger = ledger();
        let mut value = Clock {
            version: 1,
            high_water_ms: 100,
        };
        store::write(&ledger, |tx| put_clock(tx, &value)).unwrap();
        assert_eq!(store::read(&ledger, clock).unwrap(), Some(value.clone()));
        value.high_water_ms = 200;
        store::write(&ledger, |tx| put_clock(tx, &value)).unwrap();
        assert_eq!(store::read(&ledger, clock).unwrap(), Some(value.clone()));
        assert!(
            store::write(&ledger, |tx| put_clock(
                tx,
                &Clock {
                    version: 1,
                    high_water_ms: u64::MAX
                }
            ))
            .is_err()
        );
        assert_eq!(store::read(&ledger, clock).unwrap(), Some(value));
        std::fs::remove_dir_all(ledger.dir).unwrap();
    }

    #[test]
    fn selection_roundtrip_update_remove_and_invalid_id() {
        let ledger = ledger();
        put_selection(&ledger, "request", Some("host")).unwrap();
        assert_eq!(
            selection(&ledger, "request").unwrap().as_deref(),
            Some("host")
        );
        put_selection(&ledger, "request", Some("other")).unwrap();
        assert_eq!(
            selection(&ledger, "request").unwrap().as_deref(),
            Some("other")
        );
        assert!(put_selection(&ledger, "bad/id", Some("host")).is_err());
        assert!(selection(&ledger, "bad/id").unwrap().is_none());
        put_selection(&ledger, "request", None).unwrap();
        assert!(selection(&ledger, "request").unwrap().is_none());
        std::fs::remove_dir_all(ledger.dir).unwrap();
    }

    #[test]
    fn reservation_roundtrip_order_update_and_invalid_list_rollback() {
        let ledger = ledger();
        let values = vec![
            Reservation {
                id: "second".into(),
                created_ms: 20,
            },
            Reservation {
                id: "first".into(),
                created_ms: 10,
            },
        ];
        put_reservations(&ledger, &values).unwrap();
        assert_eq!(reservations(&ledger).unwrap(), values);
        assert!(put_reservations(&ledger, &[values[0].clone(), values[0].clone()]).is_err());
        assert_eq!(reservations(&ledger).unwrap(), values);
        assert!(
            put_reservations(
                &ledger,
                &[Reservation {
                    id: "bad".into(),
                    created_ms: u64::MAX
                }]
            )
            .is_err()
        );
        assert_eq!(reservations(&ledger).unwrap(), values);
        put_reservations(&ledger, &values[1..]).unwrap();
        assert_eq!(reservations(&ledger).unwrap(), values[1..]);
        put_reservations(&ledger, &[]).unwrap();
        assert_eq!(
            reservations(&ledger).unwrap(),
            [] as [crate::ledger::Reservation; 0]
        );
        std::fs::remove_dir_all(ledger.dir).unwrap();
    }

    #[test]
    fn cancellation_roundtrip_preserves_evidence_and_clears_children() {
        let ledger = ledger();
        let mut value = json!({
            "version":1,"completion_version":1,"finished_ms":200,"operation_id":"operation",
            "ticket":{
                "id":"ticket","finished_ms":199,"created_ms":100,"state":"cancelled","waiting":true,
                "waiting_until_ms":150,"wait_reason":"busy","notified":true,"result":{"result":"evidence"},"error":"error",
                "retry":{"attempts":2,"next_ms":130},
                "request":{"id":"request","validity":{"version":1,"created_ms":90},
                    "action":{"type":"job","cmd":["sh","-c","true"],"timeout_ms":30,"idle_timeout_ms":20,"validity":{"version":1,"created_ms":90}},
                    "host":"local","branch":"feat/job","base":"main","cwd":"/checkout",
                    "repository":{"path":"/repo","origin":"origin","relative":"repo","working_root":"/checkout"},
                    "workspace":"workspace","workspace_name":"name","owner":"owner","owner_host":"host"}
            },
            "admission":{"request":{"id":"request"},"host":{"id":"local","label":"local","target":null}},
            "preparation":{"version":1,"request":{"id":"request","action":{"type":"place"},"cwd":"/repo","owner":"owner"},
                "host":{"id":"local","label":"local","target":null},
                "preparation":{"version":2,"request_id":"request","repo":"/repo","branch":"feat/job","base":"main"},"binding":null},
            "placement":null,"outcome":"not_started","release":"pending","intent":null,"acknowledgement":null,
            "error":"pending release","held":{"reason":"inspect","evidence":"digest"}
        });
        let binding = json!({"request_id":"request","request":["/repo","feat/job","main"],
            "claim":{"request_id":"request","tree":{"repo":"/repo","path":"/checkout","branch":"feat/job","head":"head"}},
            "prepared":{"pane_id":"pane","terminal_id":"terminal","workspace_id":"workspace","path":"/checkout"}});
        let policy = json!({"kind":"prohibit","basis":{"cancelled":{"operation_id":"operation","request_id":"request"}}});
        value["intent"] =
            json!({"version":1,"binding":binding,"policy":policy,"outcome":"not_started"});
        value["acknowledgement"] = json!({"version":1,"binding":binding,"policy":policy,"outcome":"not_started","released":true});
        value["preparation"]["binding"] = binding.clone();
        value["placement"] = json!({"version":1,
            "placement":{"version":3,"request":value["ticket"]["request"],"host":{"id":"local","label":"local","target":null},"reservation":"request","expected_grant":null},
            "preparation":value["preparation"]["preparation"],"binding":binding});
        value["ticket"]["diagnostic"] = json!({"version":1,"operation":"prepare","cause":"setup_failed",
            "context":{"branch":"feat/job","base":"main","workspace":"/checkout","occupancy":"locked","new_worktree_removed":true},
            "status":17,"host":"local","request_id":"request","omission":"invalid_envelope"});
        let mut saved: crate::work::Cancellation = serde_json::from_value(value).unwrap();
        put_cancellation(&ledger, &saved).unwrap();
        assert_eq!(
            serde_json::to_value(cancellation(&ledger, "request").unwrap().unwrap()).unwrap(),
            serde_json::to_value(&saved).unwrap()
        );
        let revision = cancellation_revision(&ledger, "request").unwrap();
        saved.ticket.request.action = crate::work::Action::Place;
        saved.admission = None;
        saved.preparation = None;
        saved.placement = None;
        saved.ticket.diagnostic = None;
        saved.intent = None;
        saved.acknowledgement = None;
        saved.held = None;
        saved.error = None;
        put_cancellation(&ledger, &saved).unwrap();
        assert!(cancellation_revision(&ledger, "request").unwrap() > revision);
        let args = store::read(&ledger, |tx| {
            list(
                tx,
                "SELECT arg FROM cancellation_job_args WHERE cancellation = ?1 ORDER BY position",
                "request",
                |row| row.get::<_, String>(0),
            )
        })
        .unwrap();
        assert_eq!(args, [] as [String; 0]);
        assert_eq!(
            serde_json::to_value(cancellation(&ledger, "request").unwrap().unwrap()).unwrap(),
            serde_json::to_value(&saved).unwrap()
        );
        saved.operation_id = "bad/id".into();
        assert!(put_cancellation(&ledger, &saved).is_err());
        assert_eq!(cancellations(&ledger).unwrap().len(), 1);
        saved.operation_id = "operation".into();
        saved.ticket.request.id = "invalid".into();
        saved.ticket.created_ms = u64::MAX;
        assert!(put_cancellation(&ledger, &saved).is_err());
        assert!(cancellation(&ledger, "invalid").unwrap().is_none());
        remove_cancellation(&ledger, "request").unwrap();
        assert_eq!(cancellations(&ledger).unwrap().len(), 0);
        std::fs::remove_dir_all(ledger.dir).unwrap();
    }
}
