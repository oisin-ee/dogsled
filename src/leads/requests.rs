//! Origin-owned lead start receipts. A committed dispatch claim is never replayed.

use super::{CreateContext, Failure, FailureKind, LaunchContext, Phase, Started, routing};
use crate::error::{Error, Result};
use crate::ledger::{Ledger, now_ms};
use crate::{lead_store, rows, store};
pub(crate) use portal_types::agents::LeadStartState as State;
use portal_types::agents::{AgentAddress, LeadStartView};
use portal_types::models::{ModelRef, ThinkingLevel};
use rusqlite::{Row, Transaction, params};
use serde::Serialize;

pub(crate) const SCHEMA: &str = "
CREATE TABLE lead_requests (
    id TEXT PRIMARY KEY,
    key TEXT NOT NULL UNIQUE CHECK (length(key) BETWEEN 1 AND 128),
    delivery TEXT NOT NULL CHECK (json_valid(delivery)),
    state TEXT NOT NULL CHECK (state IN ('accepted', 'dispatching', 'ready', 'failed', 'uncertain')),
    revision INTEGER NOT NULL CHECK (revision > 0),
    host TEXT NOT NULL CHECK (length(host) > 0),
    name TEXT,
    pane TEXT,
    workspace TEXT,
    failure_kind TEXT CHECK (failure_kind IN ('invalid', 'unavailable', 'launch')),
    phase TEXT CHECK (phase IN ('recorded', 'allocating', 'allocated', 'starting', 'started', 'prompting', 'ready')),
    error TEXT,
    created_ms INTEGER NOT NULL CHECK (created_ms >= 0),
    updated_ms INTEGER NOT NULL CHECK (updated_ms >= 0)
) STRICT;
CREATE INDEX lead_requests_work ON lead_requests (state, created_ms, id);
";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Status {
    pub id: String,
    pub revision: u64,
    pub state: State,
    pub host: String,
    pub name: Option<String>,
    pub pane: Option<String>,
    pub workspace: Option<String>,
    pub failure_kind: Option<FailureKind>,
    pub phase: Option<Phase>,
    pub error: Option<String>,
    pub created_ms: u64,
}

impl Status {
    pub(crate) fn view(self) -> Result<LeadStartView> {
        Ok(LeadStartView {
            id: self.id,
            revision: self.revision,
            state: self.state,
            name: self.name,
            address: self.pane.map(|pane| AgentAddress {
                host: self.host,
                pane,
            }),
            phase: self.phase.as_ref().map(rows::name).transpose()?,
            error: self.error,
            created_ms: self.created_ms,
        })
    }
}

/// Snapshot only origin receipts, never the saved delivery or its prompt.
pub(crate) fn recent(ledger: &Ledger, now: u64) -> Result<Vec<LeadStartView>> {
    let cutoff = rows::integer(now.saturating_sub(10 * 60 * 1000))?;
    store::read(ledger, |tx| {
        let mut statement = tx.prepare_cached(
            "SELECT r.*, l.name AS live_name, l.pane AS live_pane, l.phase AS live_phase
             FROM lead_requests r LEFT JOIN leads l
               ON r.state = 'dispatching' AND r.host = ?2 AND l.id = r.id
             WHERE r.state IN ('accepted', 'dispatching') OR r.updated_ms >= ?1
             ORDER BY r.created_ms DESC, r.id DESC LIMIT 50",
        )?;
        let mut results = statement.query(params![cutoff, crate::hosts::LOCAL])?;
        let mut views = Vec::new();
        while let Some(row) = results.next()? {
            let mut status = load_status(row)?;
            if let Some(name) = rows::text(row, "live_name")? {
                status.name = Some(name);
                status.pane = rows::text(row, "live_pane")?;
                status.phase = rows::text(row, "live_phase")?
                    .map(rows::variant)
                    .transpose()?;
            }
            views.push(status.view()?);
        }
        Ok(views)
    })
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum AcceptFailure {
    #[error("lead request key must contain 1 to 128 visible ASCII characters")]
    InvalidKey,
    #[error("lead request key is already in use with different inputs")]
    Conflict,
    #[error("{0}")]
    Placement(Failure),
    #[error(transparent)]
    Store(#[from] Error),
}

/// Replays read their receipt without consulting the publication. The origin lock
/// serializes acceptance across processes, without holding SQLite open during placement.
pub(crate) fn accept(
    context: &CreateContext<'_>,
    key: &str,
    slug: &str,
    prompt: &str,
    model: Option<ModelRef>,
    thinking: Option<ThinkingLevel>,
) -> std::result::Result<Status, AcceptFailure> {
    if key.is_empty() || key.len() > 128 || !key.bytes().all(|byte| byte.is_ascii_graphic()) {
        return Err(AcceptFailure::InvalidKey);
    }
    let ledger = context.ledger;
    ledger.with_lock("lead-accept", || {
        let saved = store::read(ledger, |tx| find_key(tx, key))?;
        if let Some((status, delivery)) = saved {
            return Ok(
                if delivery.repo == slug
                    && delivery.prompt == prompt
                    && delivery.model == model
                    && delivery.thinking == thinking
                {
                    Ok(status)
                } else {
                    Err(AcceptFailure::Conflict)
                },
            );
        }
        let placed = match routing::place(context, slug, prompt, model, thinking) {
            Ok(placed) => placed,
            Err(error) => return Ok(Err(AcceptFailure::Placement(error))),
        };
        let status = store::write(ledger, |tx| {
            let delivery = placed.record_in(tx)?;
            insert(tx, key, &delivery)?;
            read_status(tx, &delivery.id)?.ok_or_else(|| missing(&delivery.id))
        })?;
        Ok(Ok(status))
    })?
}

fn insert(tx: &Transaction<'_>, key: &str, delivery: &routing::Delivery) -> Result<()> {
    let now = rows::integer(now_ms())?;
    tx.execute(
        "INSERT INTO lead_requests (id, key, delivery, state, revision, host, created_ms, updated_ms)
         VALUES (?1, ?2, ?3, 'accepted', 1, ?4, ?5, ?5)",
        params![delivery.id, key, serde_json::to_string(delivery)?, delivery.host, now],
    )?;
    Ok(())
}

#[cfg(test)]
pub(crate) fn status(ledger: &Ledger, id: &str) -> Result<Option<Status>> {
    store::read(ledger, |tx| read_status(tx, id))
}

fn read_status(tx: &Transaction<'_>, id: &str) -> Result<Option<Status>> {
    let mut statement = tx.prepare_cached("SELECT * FROM lead_requests WHERE id = ?1")?;
    let mut results = statement.query([id])?;
    results.next()?.map(load_status).transpose()
}

fn find_key(tx: &Transaction<'_>, key: &str) -> Result<Option<(Status, routing::Delivery)>> {
    let mut statement = tx.prepare_cached("SELECT * FROM lead_requests WHERE key = ?1")?;
    let mut results = statement.query([key])?;
    results
        .next()?
        .map(|row| Ok((load_status(row)?, load_delivery(row)?)))
        .transpose()
}

fn load_status(row: &Row<'_>) -> Result<Status> {
    Ok(Status {
        id: rows::required(row, "id")?,
        revision: rows::unsigned(row.get("revision")?)?,
        state: rows::variant(rows::required(row, "state")?)?,
        host: rows::required(row, "host")?,
        name: rows::text(row, "name")?,
        pane: rows::text(row, "pane")?,
        workspace: rows::text(row, "workspace")?,
        failure_kind: rows::text(row, "failure_kind")?
            .map(rows::variant)
            .transpose()?,
        phase: rows::text(row, "phase")?.map(rows::variant).transpose()?,
        error: rows::text(row, "error")?,
        created_ms: rows::unsigned(row.get("created_ms")?)?,
    })
}

fn load_delivery(row: &Row<'_>) -> Result<routing::Delivery> {
    let delivery: routing::Delivery = serde_json::from_str(&rows::required(row, "delivery")?)?;
    if delivery.id != rows::required(row, "id")? || delivery.host != rows::required(row, "host")? {
        return Err(Error::Invalid(
            "lead request delivery identity changed".into(),
        ));
    }
    Ok(delivery)
}

/// Process at most one receipt. A failed result write leaves a dispatch claim for
/// restart recovery, never an accepted row that could launch again.
pub(crate) fn work(context: &LaunchContext<'_>) -> Result<bool> {
    let ledger = context.ledger;
    let Some(delivery) = claim(ledger)? else {
        return Ok(false);
    };
    let outcome = match routing::deliver(context, &delivery) {
        Ok(lead) => Outcome::Ready(lead),
        Err(failure) => Outcome::Stopped {
            uncertain: failure.uncertain,
            kind: failure.kind,
            pane: failure.pane,
            phase: failure.phase,
            error: failure.error.to_string(),
        },
    };
    store::write(ledger, |tx| finish(tx, &delivery.id, &outcome))?;
    Ok(true)
}

fn claim(ledger: &Ledger) -> Result<Option<routing::Delivery>> {
    store::write(ledger, |tx| {
        let delivery = {
            let mut statement = tx.prepare_cached(
                "SELECT id, host, delivery FROM lead_requests WHERE state = 'accepted'
                 ORDER BY created_ms, id LIMIT 1",
            )?;
            let mut rows = statement.query([])?;
            rows.next()?.map(load_delivery).transpose()?
        };
        if let Some(delivery) = &delivery {
            tx.execute(
                "UPDATE lead_requests SET state = 'dispatching', revision = revision + 1,
                 updated_ms = ?2 WHERE id = ?1",
                params![delivery.id, rows::integer(now_ms())?],
            )?;
        }
        Ok(delivery)
    })
}

fn missing(id: &str) -> Error {
    Error::Invalid(format!("lead request {id} not found in its expected state"))
}

enum Outcome {
    Ready(Started),
    Stopped {
        uncertain: bool,
        kind: FailureKind,
        pane: Option<String>,
        phase: Option<Phase>,
        error: String,
    },
}

fn finish(tx: &Transaction<'_>, id: &str, outcome: &Outcome) -> Result<()> {
    let now = rows::integer(now_ms())?;
    let changed = match outcome {
        Outcome::Ready(lead) => tx.execute(
            "UPDATE lead_requests SET state = 'ready', revision = revision + 1, updated_ms = ?2,
             name = ?3, pane = ?4, workspace = ?5, phase = 'ready'
             WHERE id = ?1 AND state = 'dispatching' AND host = ?6",
            params![id, now, lead.name, lead.pane, lead.workspace, lead.host],
        )?,
        Outcome::Stopped {
            uncertain,
            kind,
            pane,
            phase,
            error,
        } => tx.execute(
            "UPDATE lead_requests SET state = ?3, revision = revision + 1, updated_ms = ?2,
             failure_kind = ?4, pane = ?5, phase = ?6, error = ?7
             WHERE id = ?1 AND state = 'dispatching'",
            params![
                id,
                now,
                if *uncertain { "uncertain" } else { "failed" },
                rows::name(kind)?,
                pane,
                phase.as_ref().map(rows::name).transpose()?,
                error
            ],
        )?,
    };
    if changed != 1 {
        return Err(missing(id));
    }
    Ok(())
}

/// Run once at service startup under the service's host-local lifetime lock.
/// Only persisted local launch evidence can resolve a dispatch after restart.
pub(crate) fn recover(ledger: &Ledger, local_host: &str) -> Result<()> {
    store::write(ledger, |tx| {
        let mut statement =
            tx.prepare_cached("SELECT id, host FROM lead_requests WHERE state = 'dispatching'")?;
        let requests = statement.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        for request in requests.collect::<rusqlite::Result<Vec<_>>>()? {
            let (id, host) = request;
            finish(tx, &id, &interrupted(tx, &id, host, local_host)?)?;
        }
        Ok(())
    })
}

fn interrupted(tx: &Transaction<'_>, id: &str, host: String, local_host: &str) -> Result<Outcome> {
    if host != local_host {
        return Ok(Outcome::Stopped {
            uncertain: true,
            kind: FailureKind::Launch,
            phase: None,
            pane: None,
            error:
                "the service stopped during peer delivery; launch is uncertain and was not retried"
                    .into(),
        });
    }
    Ok(match lead_store::get_in(tx, id)? {
        Some(record) if record.phase == Phase::Ready => Outcome::Ready(Started {
            host,
            id: record.id,
            name: record.name,
            pane: record
                .pane
                .ok_or_else(|| Error::Invalid(format!("ready lead {id} has no pane")))?,
            workspace: record.workspace,
        }),
        Some(record) => Outcome::Stopped {
            uncertain: false,
            kind: FailureKind::Launch,
            phase: Some(record.phase),
            pane: record.pane,
            error: record.error.unwrap_or_else(|| {
                "the service stopped during launch; the lead was not retried".into()
            }),
        },
        None => Outcome::Stopped {
            uncertain: false,
            kind: FailureKind::Launch,
            phase: None,
            pane: None,
            error: "the service stopped before the lead was recorded; no pane was started".into(),
        },
    })
}
