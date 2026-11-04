//! Host-local preparation rows. Immutable authority and equality witnesses are messages;
//! request routing, lifecycle state and timestamps have typed columns.

use rusqlite::types::Value as Sql;
use rusqlite::{Row, Transaction};
use serde::Serialize;
use serde_json::Value;

use crate::error::{Error, Result};
use crate::ledger::Ledger;
use crate::node::{GrantLifecycle, PlacementGrant};
use crate::preparation_retirement::Intent;
use crate::prepare::disposition::{self, Basis, Policy};
use crate::prepare::{Pending, PreparationLookup, PreparationReceipt, Prepared};
use crate::rows::{
    self, Columns, decoded, diagnostic_sql, list, number, request_sql, required, text,
};
use crate::store;

macro_rules! disposition_sql {
    () => { "
    disposition_version INTEGER NOT NULL CHECK (disposition_version >= 0),
    binding_request_id TEXT NOT NULL,
    binding_message BLOB NOT NULL,
    outcome TEXT NOT NULL CHECK (outcome IN ('possibly_executed','not_started','unknown')),
    policy TEXT NOT NULL CHECK (policy IN ('transfer','prohibit')),
    basis TEXT CHECK (basis IN ('placement_returned','preparation_failed','retired','cancelled','unit_refused','agent_refused')),
    pane TEXT,
    launch_id TEXT,
    launch_message BLOB,
    job_id TEXT,
    job_message BLOB,
    operation_id TEXT,
    cancellation_request_id TEXT,
    audit_request_id TEXT,
    audit_host TEXT,
    audit_owner TEXT,
    audit_recorded_ms INTEGER CHECK (audit_recorded_ms >= 0),
    audit_message BLOB,
    provenance_message BLOB,
    CHECK ((policy = 'transfer') = (pane IS NOT NULL)),
    CHECK ((policy = 'prohibit') = (basis IS NOT NULL)),"
    };
}

pub(crate) const SCHEMA: &str = concat!(
    "CREATE TABLE preparations (
    seq INTEGER PRIMARY KEY,
    id TEXT NOT NULL UNIQUE,
    request_id TEXT NOT NULL,
    repo TEXT NOT NULL,
    path TEXT NOT NULL,
    branch TEXT NOT NULL,
    head TEXT NOT NULL,
    identity_common TEXT,
    identity_gitdir TEXT,
    identity_generation TEXT,
    CHECK ((identity_common IS NULL) = (identity_gitdir IS NULL)),
    CHECK ((identity_common IS NULL) = (identity_generation IS NULL))
) STRICT;
CREATE INDEX preparations_request ON preparations (request_id);
CREATE INDEX preparations_path ON preparations (path);
CREATE TABLE preparation_receipts (
    id TEXT PRIMARY KEY,
    finished_ms INTEGER CHECK (finished_ms >= 0),
    validity_version INTEGER,
    validity_created_ms INTEGER CHECK (validity_created_ms >= 0),
    version INTEGER CHECK (version >= 0),
    binding_request_id TEXT,
    binding_message BLOB,
    repo TEXT NOT NULL,
    branch TEXT NOT NULL,
    base TEXT,
    prepared INTEGER NOT NULL CHECK (prepared IN (0,1)),
    prepared_binding_message BLOB,
    pane_id TEXT,
    terminal_id TEXT,
    workspace_id TEXT,
    path TEXT,
    failure_operation TEXT,
    failure_kind TEXT,
    failure_transport TEXT,
    failure_refusal TEXT,
    failure_request_id TEXT,
    failure_detail_withheld INTEGER CHECK (failure_detail_withheld IN (0,1)),",
    diagnostic_sql!(),
    "
    CHECK ((validity_version IS NULL) = (validity_created_ms IS NULL)),
    CHECK ((binding_request_id IS NULL) = (binding_message IS NULL)),
    CHECK (prepared = 0 OR failure_kind IS NULL)
) STRICT;
CREATE TABLE preparation_dispositions (
    id TEXT PRIMARY KEY,
    finished_ms INTEGER CHECK (finished_ms >= 0),
    progress TEXT NOT NULL CHECK (progress IN ('fenced','released')),",
    disposition_sql!(),
    "CHECK (finished_ms IS NULL OR progress = 'released')
) STRICT;
CREATE INDEX preparation_dispositions_progress ON preparation_dispositions (progress);
CREATE TABLE placement_grants (
    id TEXT PRIMARY KEY,",
    request_sql!(),
    "
    version INTEGER NOT NULL CHECK (version >= 0),
    envelope_version INTEGER NOT NULL CHECK (envelope_version >= 0),
    host_id TEXT NOT NULL,
    host_label TEXT NOT NULL,
    host_target TEXT,
    envelope_reservation TEXT NOT NULL,
    reservation_id TEXT NOT NULL,
    reservation_created_ms INTEGER NOT NULL CHECK (reservation_created_ms >= 0),
    headroom_host TEXT NOT NULL,
    headroom REAL NOT NULL,
    eligible INTEGER NOT NULL CHECK (eligible IN (0,1)),
    reason TEXT,
    live_agents INTEGER NOT NULL CHECK (live_agents >= 0),
    materialized INTEGER NOT NULL CHECK (materialized IN (0,1))
) STRICT;
CREATE TABLE placement_grant_args (
    grant_id TEXT NOT NULL REFERENCES placement_grants (id) ON DELETE CASCADE,
    position INTEGER NOT NULL CHECK (position >= 0),
    arg TEXT NOT NULL,
    PRIMARY KEY (grant_id, position)
) STRICT, WITHOUT ROWID;
CREATE TABLE placement_grant_lifecycles (
    id TEXT PRIMARY KEY,
    version INTEGER NOT NULL CHECK (version >= 0),
    finished_ms INTEGER CHECK (finished_ms >= 0),
    grant_message BLOB NOT NULL,
    admission_message BLOB NOT NULL
) STRICT;
CREATE TABLE preparation_expectations (
    id TEXT PRIMARY KEY,",
    request_sql!(),
    "
    version INTEGER NOT NULL CHECK (version >= 0),
    host_id TEXT NOT NULL,
    host_label TEXT NOT NULL,
    host_target TEXT,
    preparation_version INTEGER NOT NULL CHECK (preparation_version >= 0),
    preparation_request_id TEXT NOT NULL,
    preparation_repo TEXT NOT NULL,
    preparation_branch TEXT NOT NULL,
    preparation_base TEXT,
    preparation_validity_version INTEGER,
    preparation_validity_created_ms INTEGER,
    binding_request_id TEXT,
    binding_message BLOB,
    CHECK ((preparation_validity_version IS NULL) = (preparation_validity_created_ms IS NULL))
) STRICT;
CREATE TABLE preparation_expectation_args (
    expectation TEXT NOT NULL REFERENCES preparation_expectations (id) ON DELETE CASCADE,
    position INTEGER NOT NULL CHECK (position >= 0),
    arg TEXT NOT NULL,
    PRIMARY KEY (expectation, position)
) STRICT, WITHOUT ROWID;
CREATE TABLE preparation_retirements (
    id TEXT PRIMARY KEY,",
    // Historical retirement can predate an originating request.
    "request_present INTEGER NOT NULL CHECK (request_present IN (0,1)),
    request_id TEXT,
    request_validity_version INTEGER,
    request_validity_created_ms INTEGER,
    action TEXT CHECK (action IN ('place','job','agent')),
    job_timeout_ms INTEGER,
    job_idle_timeout_ms INTEGER,
    job_validity_version INTEGER,
    job_validity_created_ms INTEGER,
    agent_kind TEXT,
    agent_name TEXT,
    agent_message TEXT,
    agent_profile TEXT,
    request_host TEXT,
    request_branch TEXT,
    request_base TEXT,
    request_cwd TEXT,
    repository_path TEXT,
    repository_origin TEXT,
    repository_relative TEXT,
    repository_working_root TEXT,
    request_workspace TEXT,
    request_workspace_name TEXT,
    request_owner TEXT,
    request_owner_host TEXT,
    version INTEGER NOT NULL CHECK (version >= 0),
    acknowledgement_message BLOB,
    error TEXT,",
    disposition_sql!(),
    "CHECK ((request_present = 1) = (request_id IS NOT NULL))
) STRICT;
CREATE INDEX preparation_retirements_pending ON preparation_retirements (id)
    WHERE acknowledgement_message IS NULL;
CREATE TABLE preparation_retirement_args (
    retirement TEXT NOT NULL REFERENCES preparation_retirements (id) ON DELETE CASCADE,
    position INTEGER NOT NULL CHECK (position >= 0),
    arg TEXT NOT NULL,
    PRIMARY KEY (retirement, position)
) STRICT, WITHOUT ROWID;"
);

fn invalid(message: &str) -> Error {
    Error::Invalid(message.into())
}
fn version(row: &Row<'_>, column: &str) -> Result<u32> {
    u32::try_from(row.get::<_, i64>(column)?).map_err(|_| invalid("invalid preparation version"))
}
fn value<T: Serialize>(record: Option<T>) -> Result<Option<Value>> {
    record
        .map(|record| Ok(serde_json::to_value(record)?))
        .transpose()
}

macro_rules! records {
    ($module:ident, $table:literal, $type:ty, $save:ident, $load:ident) => {
        pub(crate) mod $module {
            use super::*;
            pub(crate) fn get(ledger: &Ledger, id: &str) -> Result<Option<$type>> {
                store::read(ledger, |tx| {
                    let mut statement =
                        tx.prepare_cached(concat!("SELECT * FROM ", $table, " WHERE id = ?1"))?;
                    let mut rows = statement.query([id])?;
                    rows.next()?.map(|row| $load(tx, row)).transpose()
                })
            }
            pub(crate) fn put(ledger: &Ledger, id: &str, record: &$type) -> Result<()> {
                crate::jobs::validate_id(id)?;
                store::write(ledger, |tx| $save(tx, id, record))
            }
            pub(crate) fn value(ledger: &Ledger, id: &str) -> Result<Option<Value>> {
                super::value(get(ledger, id)?)
            }
            pub(crate) fn remove(ledger: &Ledger, id: &str) -> Result<()> {
                store::write(ledger, |tx| {
                    tx.execute(concat!("DELETE FROM ", $table, " WHERE id = ?1"), [id])?;
                    Ok(())
                })
            }
            pub(crate) fn ids(ledger: &Ledger) -> Result<Vec<String>> {
                store::read(ledger, |tx| {
                    Ok(tx
                        .prepare_cached(concat!("SELECT id FROM ", $table, " ORDER BY id"))?
                        .query_map([], |row| row.get(0))?
                        .collect::<rusqlite::Result<_>>()?)
                })
            }
            pub(crate) fn all(ledger: &Ledger) -> Result<Vec<$type>> {
                store::read(ledger, |tx| {
                    let mut statement =
                        tx.prepare_cached(concat!("SELECT * FROM ", $table, " ORDER BY id"))?;
                    let mut rows = statement.query([])?;
                    let mut records = Vec::new();
                    while let Some(row) = rows.next()? {
                        records.push($load(tx, row)?);
                    }
                    Ok(records)
                })
            }
        }
    };
}

records!(
    receipts,
    "preparation_receipts",
    PreparationReceipt,
    save_receipt,
    load_receipt
);
records!(
    dispositions,
    "preparation_dispositions",
    disposition::Receipt,
    save_disposition,
    load_disposition
);
records!(
    grants,
    "placement_grants",
    PlacementGrant,
    save_grant,
    load_grant
);
records!(
    lifecycles,
    "placement_grant_lifecycles",
    GrantLifecycle,
    save_lifecycle,
    load_lifecycle
);
records!(
    expectations,
    "preparation_expectations",
    PreparationLookup,
    save_expectation,
    load_expectation
);
records!(
    retirements,
    "preparation_retirements",
    Intent,
    save_retirement,
    load_retirement
);

pub(crate) fn pending(ledger: &Ledger) -> Result<Vec<Pending>> {
    store::read(ledger, pending_rows)
}

pub(crate) fn pending_for(ledger: &Ledger, id: &str) -> Result<Vec<Pending>> {
    store::read(ledger, |tx| pending_request_rows(tx, id))
}
pub(crate) fn pending_request_rows(tx: &Transaction<'_>, id: &str) -> Result<Vec<Pending>> {
    select_pending(tx, Some(id))
}
pub(crate) fn pending_rows(tx: &Transaction<'_>) -> Result<Vec<Pending>> {
    select_pending(tx, None)
}
fn select_pending(tx: &Transaction<'_>, id: Option<&str>) -> Result<Vec<Pending>> {
    let sql = if id.is_some() {
        "SELECT * FROM preparations WHERE request_id = ?1 ORDER BY seq"
    } else {
        "SELECT * FROM preparations ORDER BY seq"
    };
    let mut statement = tx.prepare_cached(sql)?;
    let mut rows = statement.query(rusqlite::params_from_iter(id))?;
    let mut result = Vec::new();
    while let Some(row) = rows.next()? {
        let identity = match text(row, "identity_common")? {
            Some(common) => Some(crate::worktree::Identity {
                common: common.into(),
                gitdir: required(row, "identity_gitdir")?.into(),
                generation: required(row, "identity_generation")?,
            }),
            None => None,
        };
        result.push(Pending {
            request_id: required(row, "request_id")?,
            tree: crate::cleanup::Tree {
                repo: required(row, "repo")?,
                path: required(row, "path")?,
                branch: required(row, "branch")?,
                head: required(row, "head")?,
                identity,
            },
        });
    }
    Ok(result)
}

pub(crate) fn put_pending(ledger: &Ledger, records: &[Pending]) -> Result<()> {
    store::write(ledger, |tx| save_pending(tx, records))
}

pub(crate) fn save_pending(tx: &Transaction<'_>, records: &[Pending]) -> Result<()> {
    tx.execute("DELETE FROM preparations", [])?;
    for record in records {
        save_pending_row(tx, record)?;
    }
    Ok(())
}

fn save_pending_row(tx: &Transaction<'_>, record: &Pending) -> Result<()> {
    crate::jobs::validate_id(&record.request_id)?;
    let mut c = Columns::default();
    c.text("id", Some(&crate::ledger::new_id()));
    c.text("request_id", Some(&record.request_id));
    c.text("repo", Some(&record.tree.repo));
    c.text("path", Some(&record.tree.path));
    c.text("branch", Some(&record.tree.branch));
    c.text("head", Some(&record.tree.head));
    if let Some(identity) = &record.tree.identity {
        c.text("identity_common", Some(&identity.common.to_string_lossy()));
        c.text("identity_gitdir", Some(&identity.gitdir.to_string_lossy()));
        c.text("identity_generation", Some(&identity.generation));
    }
    c.upsert(tx, "preparations", "id")
}

pub(crate) fn record_pending(ledger: &Ledger, record: &Pending) -> Result<()> {
    let mut record = record.clone();
    record.tree.identity = record
        .tree
        .identity
        .map(crate::worktree::Identity::normalize_local)
        .transpose()?;
    store::write(ledger, |tx| {
        for old in pending_request_rows(tx, &record.request_id)? {
            if crate::worktree::same(&old.tree.path, &record.tree.path) {
                tx.execute(
                    "DELETE FROM preparations WHERE request_id = ?1 AND path = ?2",
                    rusqlite::params![record.request_id, old.tree.path],
                )?;
            }
        }
        save_pending_row(tx, &record)
    })
}

pub(crate) fn release_pending(
    ledger: &Ledger,
    claim: &disposition::Claim,
    resume: bool,
) -> Result<()> {
    store::write(ledger, |tx| {
        let records = pending_request_rows(tx, &claim.request_id)?;
        match records.as_slice() {
            [record] if record.tree == claim.tree => {
                tx.execute(
                    "DELETE FROM preparations WHERE request_id = ?1",
                    [&claim.request_id],
                )?;
                Ok(())
            }
            [] if resume => Ok(()),
            _ => Err(crate::worktree::refuse(
                "preparation claim changed or disappeared",
            )),
        }
    })
}

fn save_receipt(tx: &Transaction<'_>, id: &str, receipt: &PreparationReceipt) -> Result<()> {
    let mut c = Columns::default();
    c.text("id", Some(id));
    c.number("finished_ms", receipt.finished_ms)?;
    c.number(
        "validity_version",
        receipt.validity.map(|v| u64::from(v.version)),
    )?;
    c.number(
        "validity_created_ms",
        receipt.validity.map(|v| v.created_ms),
    )?;
    c.number("version", receipt.version.map(u64::from))?;
    c.text(
        "binding_request_id",
        receipt.binding.as_ref().map(|b| b.request_id.as_str()),
    );
    c.message("binding_message", receipt.binding.as_ref())?;
    c.text("repo", Some(&receipt.request.0.to_string_lossy()));
    c.text("branch", Some(&receipt.request.1));
    c.text("base", receipt.request.2.as_deref());
    c.flag("prepared", receipt.prepared.is_some());
    if let Some(prepared) = &receipt.prepared {
        c.message("prepared_binding_message", prepared.binding.as_ref())?;
        c.text("pane_id", prepared.pane_id.as_deref());
        c.text("terminal_id", prepared.terminal_id.as_deref());
        c.text("workspace_id", prepared.workspace_id.as_deref());
        c.text("path", prepared.path.as_deref());
    }
    if let Some(failure) = &receipt.failure {
        c.text("failure_operation", Some(&rows::name(&failure.operation)?));
        c.text("failure_kind", Some(&rows::name(&failure.kind)?));
        c.text(
            "failure_transport",
            rows::optional(failure.transport.as_ref(), rows::name)?.as_deref(),
        );
        c.text(
            "failure_refusal",
            rows::optional(failure.refusal.as_ref(), rows::name)?.as_deref(),
        );
        c.text("failure_request_id", failure.request_id.as_deref());
        c.flag("failure_detail_withheld", failure.detail_withheld);
        rows::diagnostic_columns(&mut c, failure.diagnostic.as_ref())?;
    }
    c.upsert(tx, "preparation_receipts", "id")
}

fn load_receipt(_tx: &Transaction<'_>, row: &Row<'_>) -> Result<PreparationReceipt> {
    let failure = match text(row, "failure_kind")? {
        Some(kind) => Some(crate::error::RetainedFailure {
            operation: rows::variant(required(row, "failure_operation")?)?,
            kind: rows::variant(kind)?,
            transport: rows::optional(text(row, "failure_transport")?, rows::variant)?,
            refusal: rows::optional(text(row, "failure_refusal")?, rows::variant)?,
            request_id: text(row, "failure_request_id")?,
            detail_withheld: row.get("failure_detail_withheld")?,
            diagnostic: rows::load_diagnostic(row)?,
        }),
        None => None,
    };
    Ok(PreparationReceipt {
        finished_ms: number(row, "finished_ms")?,
        validity: rows::validity(
            row.get("validity_version")?,
            row.get("validity_created_ms")?,
        )?,
        version: row
            .get::<_, Option<i64>>("version")?
            .map(|v| u32::try_from(v).map_err(|_| invalid("invalid receipt version")))
            .transpose()?,
        binding: decoded(row, "binding_message")?,
        request: (
            required(row, "repo")?.into(),
            required(row, "branch")?,
            text(row, "base")?,
        ),
        prepared: if row.get::<_, bool>("prepared")? {
            Some(Prepared {
                binding: decoded(row, "prepared_binding_message")?,
                pane_id: text(row, "pane_id")?,
                terminal_id: text(row, "terminal_id")?,
                workspace_id: text(row, "workspace_id")?,
                path: text(row, "path")?,
            })
        } else {
            None
        },
        failure,
    })
}

fn disposition_columns(c: &mut Columns, request: &disposition::Request) -> Result<()> {
    c.number("disposition_version", Some(u64::from(request.version)))?;
    c.text("binding_request_id", Some(&request.binding.request_id));
    c.message("binding_message", Some(&request.binding))?;
    c.text("outcome", Some(&rows::name(&request.outcome)?));
    match &request.policy {
        Policy::Transfer { launch, pane, job } => {
            c.text("policy", Some("transfer"));
            c.text("pane", Some(pane));
            c.text("launch_id", Some(&launch.id));
            c.message("launch_message", Some(launch))?;
            c.text("job_id", job.as_ref().map(|j| j.id.as_str()));
            c.message("job_message", job.as_ref())?;
        }
        Policy::Prohibit { basis } => {
            c.text("policy", Some("prohibit"));
            let name = match basis {
                Basis::PlacementReturned => "placement_returned",
                Basis::PreparationFailed => "preparation_failed",
                Basis::Cancelled {
                    operation_id,
                    request_id,
                } => {
                    c.text("operation_id", Some(operation_id));
                    c.text("cancellation_request_id", Some(request_id));
                    "cancelled"
                }
                Basis::UnitRefused { launch } | Basis::AgentRefused { launch } => {
                    c.text("launch_id", Some(&launch.id));
                    c.message("launch_message", Some(launch))?;
                    if matches!(basis, Basis::UnitRefused { .. }) {
                        "unit_refused"
                    } else {
                        "agent_refused"
                    }
                }
                Basis::Retired { audit, provenance } => {
                    c.text("operation_id", Some(&audit.operation_id));
                    c.text("audit_request_id", Some(&audit.request_id));
                    c.text("audit_host", Some(&audit.host.id));
                    c.text("audit_owner", audit.actor.pane.as_deref());
                    c.number("audit_recorded_ms", Some(audit.recorded_ms))?;
                    c.message("audit_message", Some(audit))?;
                    c.message("provenance_message", provenance.as_ref())?;
                    "retired"
                }
            };
            c.text("basis", Some(name));
        }
    }
    Ok(())
}

fn load_disposition_request(row: &Row<'_>) -> Result<disposition::Request> {
    let witness =
        |column| decoded(row, column)?.ok_or_else(|| invalid("missing disposition witness"));
    let policy = if required(row, "policy")? == "transfer" {
        Policy::Transfer {
            launch: Box::new(witness("launch_message")?),
            pane: required(row, "pane")?,
            job: decoded(row, "job_message")?,
        }
    } else {
        Policy::Prohibit {
            basis: match required(row, "basis")?.as_str() {
                "placement_returned" => Basis::PlacementReturned,
                "preparation_failed" => Basis::PreparationFailed,
                "cancelled" => Basis::Cancelled {
                    operation_id: required(row, "operation_id")?,
                    request_id: required(row, "cancellation_request_id")?,
                },
                "unit_refused" => Basis::UnitRefused {
                    launch: Box::new(witness("launch_message")?),
                },
                "agent_refused" => Basis::AgentRefused {
                    launch: Box::new(witness("launch_message")?),
                },
                "retired" => Basis::Retired {
                    audit: decoded(row, "audit_message")?
                        .ok_or_else(|| invalid("missing retirement audit"))?,
                    provenance: decoded(row, "provenance_message")?,
                },
                _ => return Err(invalid("invalid disposition basis")),
            },
        }
    };
    Ok(disposition::Request {
        version: version(row, "disposition_version")?,
        binding: decoded(row, "binding_message")?
            .ok_or_else(|| invalid("missing preparation binding"))?,
        policy,
        outcome: rows::variant(required(row, "outcome")?)?,
    })
}

fn save_disposition(tx: &Transaction<'_>, id: &str, receipt: &disposition::Receipt) -> Result<()> {
    let mut c = Columns::default();
    c.text("id", Some(id));
    c.number("finished_ms", receipt.finished_ms)?;
    c.text("progress", Some(&rows::name(&receipt.progress)?));
    disposition_columns(&mut c, &receipt.request)?;
    c.upsert(tx, "preparation_dispositions", "id")
}
fn load_disposition(_tx: &Transaction<'_>, row: &Row<'_>) -> Result<disposition::Receipt> {
    Ok(disposition::Receipt {
        finished_ms: number(row, "finished_ms")?,
        progress: rows::variant(required(row, "progress")?)?,
        request: load_disposition_request(row)?,
    })
}

fn save_args(
    tx: &Transaction<'_>,
    table: &str,
    owner: &str,
    id: &str,
    args: Vec<String>,
) -> Result<()> {
    rows::replace_list(tx, table, owner, "arg", id, args.into_iter().map(Sql::Text))
}
fn load_args(tx: &Transaction<'_>, table: &str, owner: &str, row: &Row<'_>) -> Result<Vec<String>> {
    list(
        tx,
        &format!("SELECT arg FROM {table} WHERE {owner} = ?1 ORDER BY position"),
        &required(row, "id")?,
        |row| row.get(0),
    )
}
fn host_columns(c: &mut Columns, host: &crate::hosts::Host) {
    c.text("host_id", Some(&host.id));
    c.text("host_label", Some(&host.label));
    c.text("host_target", host.target.as_deref());
}
fn load_host(row: &Row<'_>) -> Result<crate::hosts::Host> {
    Ok(crate::hosts::Host {
        id: required(row, "host_id")?,
        label: required(row, "host_label")?,
        target: text(row, "host_target")?,
    })
}
fn save_grant(tx: &Transaction<'_>, id: &str, grant: &PlacementGrant) -> Result<()> {
    let request = serde_json::from_value(grant.envelope.request.clone())?;
    let mut c = Columns::default();
    c.text("id", Some(id));
    let args = rows::request_columns(&mut c, &request)?;
    c.number("version", Some(u64::from(grant.version)))?;
    c.number("envelope_version", Some(u64::from(grant.envelope.version)))?;
    host_columns(&mut c, &grant.envelope.host);
    c.text("envelope_reservation", Some(&grant.envelope.reservation));
    c.text("reservation_id", Some(&grant.reservation.id));
    c.number("reservation_created_ms", Some(grant.reservation.created_ms))?;
    c.text("headroom_host", Some(&grant.headroom.host));
    if !grant.headroom.headroom.is_finite() {
        return Err(invalid("nonfinite placement headroom"));
    }
    c.put("headroom", Sql::Real(grant.headroom.headroom));
    c.flag("eligible", grant.headroom.eligible);
    c.text("reason", grant.headroom.reason.as_deref());
    c.number("live_agents", Some(grant.headroom.live_agents as u64))?;
    c.flag("materialized", grant.materialized);
    c.upsert(tx, "placement_grants", "id")?;
    save_args(tx, "placement_grant_args", "grant_id", id, args)
}
fn load_grant(tx: &Transaction<'_>, row: &Row<'_>) -> Result<PlacementGrant> {
    Ok(PlacementGrant {
        version: version(row, "version")?,
        envelope: crate::node::PlacementReservation {
            version: version(row, "envelope_version")?,
            request: serde_json::to_value(rows::load_request(row, || {
                load_args(tx, "placement_grant_args", "grant_id", row)
            })?)?,
            host: load_host(row)?,
            reservation: required(row, "envelope_reservation")?,
        },
        reservation: crate::ledger::Reservation {
            id: required(row, "reservation_id")?,
            created_ms: number(row, "reservation_created_ms")?
                .ok_or_else(|| invalid("missing reservation time"))?,
        },
        headroom: crate::plan::Headroom {
            host: required(row, "headroom_host")?,
            headroom: row.get("headroom")?,
            eligible: row.get("eligible")?,
            reason: text(row, "reason")?,
            live_agents: usize::try_from(row.get::<_, i64>("live_agents")?)
                .map_err(|_| invalid("invalid agent count"))?,
        },
        materialized: row.get("materialized")?,
    })
}
fn save_lifecycle(tx: &Transaction<'_>, id: &str, lifecycle: &GrantLifecycle) -> Result<()> {
    let mut c = Columns::default();
    c.text("id", Some(id));
    c.number("version", Some(u64::from(lifecycle.proof.version)))?;
    c.number("finished_ms", lifecycle.finished_ms)?;
    c.message("grant_message", Some(&lifecycle.proof.grant))?;
    c.message("admission_message", Some(&lifecycle.proof.admission))?;
    c.upsert(tx, "placement_grant_lifecycles", "id")
}
fn load_lifecycle(_tx: &Transaction<'_>, row: &Row<'_>) -> Result<GrantLifecycle> {
    Ok(GrantLifecycle {
        finished_ms: number(row, "finished_ms")?,
        proof: crate::node::GrantCompletion {
            version: version(row, "version")?,
            grant: decoded(row, "grant_message")?
                .ok_or_else(|| invalid("missing grant witness"))?,
            admission: decoded(row, "admission_message")?
                .ok_or_else(|| invalid("missing admission witness"))?,
        },
    })
}
fn save_expectation(tx: &Transaction<'_>, id: &str, query: &PreparationLookup) -> Result<()> {
    let mut c = Columns::default();
    c.text("id", Some(id));
    let args = rows::request_columns(&mut c, &query.request)?;
    c.number("version", Some(u64::from(query.version)))?;
    host_columns(&mut c, &query.host);
    c.number(
        "preparation_version",
        Some(u64::from(query.preparation.version)),
    )?;
    c.text(
        "preparation_request_id",
        Some(&query.preparation.request_id),
    );
    c.text("preparation_repo", Some(&query.preparation.repo));
    c.text("preparation_branch", Some(&query.preparation.branch));
    c.text("preparation_base", query.preparation.base.as_deref());
    c.number(
        "preparation_validity_version",
        query.preparation.validity.map(|v| u64::from(v.version)),
    )?;
    c.number(
        "preparation_validity_created_ms",
        query.preparation.validity.map(|v| v.created_ms),
    )?;
    c.text(
        "binding_request_id",
        query.binding.as_ref().map(|b| b.request_id.as_str()),
    );
    c.message("binding_message", query.binding.as_ref())?;
    c.upsert(tx, "preparation_expectations", "id")?;
    save_args(tx, "preparation_expectation_args", "expectation", id, args)
}
fn load_expectation(tx: &Transaction<'_>, row: &Row<'_>) -> Result<PreparationLookup> {
    Ok(PreparationLookup {
        version: version(row, "version")?,
        request: rows::load_request(row, || {
            load_args(tx, "preparation_expectation_args", "expectation", row)
        })?,
        host: load_host(row)?,
        preparation: crate::prepare::Envelope {
            version: version(row, "preparation_version")?,
            request_id: required(row, "preparation_request_id")?,
            repo: required(row, "preparation_repo")?,
            branch: required(row, "preparation_branch")?,
            base: text(row, "preparation_base")?,
            validity: rows::validity(
                row.get("preparation_validity_version")?,
                row.get("preparation_validity_created_ms")?,
            )?,
        },
        binding: decoded(row, "binding_message")?,
    })
}
fn save_retirement(tx: &Transaction<'_>, id: &str, intent: &Intent) -> Result<()> {
    let mut c = Columns::default();
    c.text("id", Some(id));
    c.number("version", Some(u64::from(intent.version)))?;
    c.flag("request_present", intent.request.is_some());
    let args = if let Some(request) = &intent.request {
        rows::request_columns(&mut c, &serde_json::from_value(request.clone())?)?
    } else {
        Vec::new()
    };
    disposition_columns(&mut c, &intent.disposition)?;
    c.message("acknowledgement_message", intent.acknowledgement.as_ref())?;
    c.text("error", intent.error.as_deref());
    c.upsert(tx, "preparation_retirements", "id")?;
    save_args(tx, "preparation_retirement_args", "retirement", id, args)
}
fn load_retirement(tx: &Transaction<'_>, row: &Row<'_>) -> Result<Intent> {
    Ok(Intent {
        version: version(row, "version")?,
        request: if row.get::<_, bool>("request_present")? {
            Some(serde_json::to_value(rows::load_request(row, || {
                load_args(tx, "preparation_retirement_args", "retirement", row)
            })?)?)
        } else {
            None
        },
        disposition: load_disposition_request(row)?,
        acknowledgement: decoded(row, "acknowledgement_message")?,
        error: text(row, "error")?,
    })
}

/// Typed record projections for diagnostics and retention; no filesystem scan.
pub(crate) fn values(ledger: &Ledger) -> Result<Vec<Value>> {
    let mut result = Vec::new();
    for value in receipts::all(ledger)? {
        result.push(serde_json::to_value(value)?);
    }
    for value in dispositions::all(ledger)? {
        result.push(serde_json::to_value(value)?);
    }
    for value in grants::all(ledger)? {
        result.push(serde_json::to_value(value)?);
    }
    for value in lifecycles::all(ledger)? {
        result.push(serde_json::to_value(value)?);
    }
    for value in expectations::all(ledger)? {
        result.push(serde_json::to_value(value)?);
    }
    for value in retirements::all(ledger)? {
        result.push(serde_json::to_value(value)?);
    }
    Ok(result)
}

pub(crate) fn exists(ledger: &Ledger, id: &str) -> Result<bool> {
    store::read(ledger, |tx| {
        Ok(tx.prepare_cached("SELECT id FROM preparation_receipts WHERE id = ?1 UNION ALL SELECT id FROM placement_grants WHERE id = ?1 UNION ALL SELECT id FROM preparation_expectations WHERE id = ?1 UNION ALL SELECT id FROM preparation_retirements WHERE id = ?1 LIMIT 1")?.exists([id])?)
    })
}

/// Integration fixtures exercise actual database errors, never legacy-file failures.
pub(crate) fn fault(ledger: &Ledger, table: &str, write: bool) -> Result<bool> {
    let sql = match (table, write) {
        ("preparation-receipts", true) => {
            "CREATE TRIGGER refuse_preparation_writes BEFORE UPDATE ON preparation_receipts BEGIN SELECT RAISE(ABORT, 'preparation write refused'); END;"
        }
        ("clear-preparation-receipts", true) => "DROP TRIGGER refuse_preparation_writes;",
        ("preparations", false) => "ALTER TABLE preparations RENAME TO preparation_read_fault;",
        ("clear-preparations", false) => {
            "ALTER TABLE preparation_read_fault RENAME TO preparations;"
        }
        _ => return Ok(false),
    };
    store::write(ledger, |tx| {
        tx.execute_batch(sql)?;
        Ok(true)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::de::DeserializeOwned;
    use serde_json::json;

    fn ledger() -> Ledger {
        Ledger::new(std::env::temp_dir().join(crate::ledger::new_id()))
    }
    fn decode<T: DeserializeOwned>(value: Value) -> T {
        serde_json::from_value(value).unwrap()
    }
    fn request() -> Value {
        json!({"id":"a","validity":{"version":1,"created_ms":10},"action":{"type":"job","cmd":["echo","ok"],"timeout_ms":20,"idle_timeout_ms":30,"validity":{"version":1,"created_ms":10}},"host":"host","branch":"branch","base":"base","cwd":"/cwd","repository":{"path":"/repo","origin":"origin","relative":"relative","working_root":"/root"},"workspace":"workspace","workspace_name":"name","owner":"owner","owner_host":"origin-host"})
    }
    fn binding() -> Value {
        json!({"request_id":"a","request":["/repo","branch","base"],"claim":{"request_id":"a","tree":{"repo":"/repo","path":"/tree","branch":"branch","head":"head","identity":{"common":"/common","gitdir":"/gitdir","generation":"generation"}}},"prepared":{"pane_id":"pane","terminal_id":"terminal","workspace_id":"workspace","path":"/tree"}})
    }
    fn grant() -> PlacementGrant {
        decode(
            json!({"version":3,"envelope":{"version":3,"request":request(),"host":{"id":"host","label":"label","target":"target"},"reservation":"a"},"reservation":{"id":"a","created_ms":10},"headroom":{"host":"host","headroom":2.5,"eligible":true,"reason":"reason","live_agents":3},"materialized":true}),
        )
    }
    fn disposition_request() -> disposition::Request {
        decode(
            json!({"version":1,"binding":binding(),"policy":{"kind":"prohibit","basis":{"retired":{"audit":{"operation_id":"operation","request_id":"a","host":{"id":"host","label":"label","target":"target"},"actor":{"host":"actor-host","user":"user","pane":"owner","session":"session"},"authority":{"kind":"cleanup"},"reason":"reason","recorded_ms":11},"provenance":{"version":1,"request":["/repo","branch","base"],"prepared":null}}}},"outcome":"unknown"}),
        )
    }
    fn assert_same<T: Serialize>(actual: &T, expected: &T) {
        assert_eq!(
            serde_json::to_value(actual).unwrap(),
            serde_json::to_value(expected).unwrap()
        );
    }

    #[test]
    fn pending_round_trip_and_atomic_replacement() {
        let ledger = ledger();
        let pending = Pending {
            request_id: "a".into(),
            tree: decode(binding()["claim"]["tree"].clone()),
        };
        put_pending(&ledger, &[pending.clone(), pending.clone()]).unwrap();
        assert_eq!(
            self::super::pending(&ledger).unwrap(),
            [pending.clone(), pending]
        );
        let invalid = Pending {
            request_id: "invalid/id".into(),
            tree: decode(binding()["claim"]["tree"].clone()),
        };
        assert!(put_pending(&ledger, &[invalid]).is_err());
        assert_eq!(self::super::pending(&ledger).unwrap().len(), 2);
        put_pending(&ledger, &[]).unwrap();
        assert_eq!(self::super::pending(&ledger).unwrap(), vec![]);
        std::fs::remove_dir_all(ledger.dir).unwrap();
    }
    #[test]
    fn receipt_round_trip_and_clearing_outcomes() {
        let ledger = ledger();
        let receipt: PreparationReceipt = decode(
            json!({"finished_ms":40,"validity":{"version":1,"created_ms":10},"version":1,"binding":binding(),"request":["/repo","branch","base"],"prepared":{"binding":binding(),"pane_id":"pane","terminal_id":"terminal","workspace_id":"workspace","path":"/tree"}}),
        );
        receipts::put(&ledger, "a", &receipt).unwrap();
        assert_same(&receipts::get(&ledger, "a").unwrap().unwrap(), &receipt);
        let failed: PreparationReceipt = decode(
            json!({"request":["/repo","branch",null],"prepared":null,"failure":{"operation":"prepare","kind":"other","transport":"other","refusal":"CLONE_FAILED","request_id":"a","detail_withheld":true,"diagnostic":{"version":1,"operation":"prepare","cause":"checkout_identity_mismatch","context":{"branch":"branch","base":"base","workspace":"workspace","new_worktree_removed":true},"status":2,"host":"host","request_id":"a","omission":"untrusted_detail"}}}),
        );
        receipts::put(&ledger, "a", &failed).unwrap();
        assert_same(&receipts::get(&ledger, "a").unwrap().unwrap(), &failed);
        let mut malformed = failed;
        malformed.finished_ms = Some(u64::MAX);
        assert!(receipts::put(&ledger, "bad", &malformed).is_err());
        assert!(receipts::get(&ledger, "bad").unwrap().is_none());
        std::fs::remove_dir_all(ledger.dir).unwrap();
    }
    #[test]
    fn disposition_round_trip_and_policy_update() {
        let ledger = ledger();
        let mut receipt = disposition::Receipt {
            finished_ms: Some(15),
            progress: disposition::Progress::Released,
            request: disposition_request(),
        };
        dispositions::put(&ledger, "a", &receipt).unwrap();
        assert_same(&dispositions::get(&ledger, "a").unwrap().unwrap(), &receipt);
        let launch: crate::work::Launch = decode(
            json!({"id":"a","action":{"type":"job","cmd":["echo","ok"]},"host":"host","cwd":"/tree","pane":"pane","workspace":"workspace","workspace_name":"name","owner":"owner"}),
        );
        let job: crate::jobs::Job = decode(
            json!({"id":"a","cmd":["echo","ok"],"cwd":"/tree","host":"host","owner":"owner","pane":"pane","state":"pending","exit":null,"signal":null,"reason":null,"started_ms":10,"ended_ms":null,"timeout_ms":20,"idle_timeout_ms":30}),
        );
        receipt.request.policy = Policy::Transfer {
            launch: Box::new(launch),
            pane: "pane".into(),
            job: Some(Box::new(job)),
        };
        dispositions::put(&ledger, "a", &receipt).unwrap();
        assert_same(&dispositions::get(&ledger, "a").unwrap().unwrap(), &receipt);
        receipt.request.policy = Policy::Prohibit {
            basis: Basis::Cancelled {
                operation_id: "operation".into(),
                request_id: "a".into(),
            },
        };
        dispositions::put(&ledger, "a", &receipt).unwrap();
        assert_same(&dispositions::get(&ledger, "a").unwrap().unwrap(), &receipt);
        receipt.finished_ms = Some(u64::MAX);
        assert!(dispositions::put(&ledger, "bad", &receipt).is_err());
        assert!(dispositions::get(&ledger, "bad").unwrap().is_none());
        std::fs::remove_dir_all(ledger.dir).unwrap();
    }
    #[test]
    fn grant_and_argument_rows_round_trip_and_clear() {
        let ledger = ledger();
        let mut grant = grant();
        grants::put(&ledger, "a", &grant).unwrap();
        assert_same(&grants::get(&ledger, "a").unwrap().unwrap(), &grant);
        grant.envelope.request["action"] = json!({"type":"place"});
        grant.envelope.host.target = None;
        grant.headroom.reason = None;
        grant.materialized = false;
        grants::put(&ledger, "a", &grant).unwrap();
        assert_same(&grants::get(&ledger, "a").unwrap().unwrap(), &grant);
        assert_eq!(
            store::read(&ledger, |tx| Ok(tx.query_row(
                "SELECT count(*) FROM placement_grant_args",
                [],
                |row| row.get::<_, i64>(0)
            )?))
            .unwrap(),
            0
        );
        grant.reservation.created_ms = u64::MAX;
        assert!(grants::put(&ledger, "bad", &grant).is_err());
        assert!(grants::get(&ledger, "bad").unwrap().is_none());
        std::fs::remove_dir_all(ledger.dir).unwrap();
    }
    #[test]
    fn grant_witness_preserves_the_sqlite_real_exactly() {
        for offset in 0..1024_u64 {
            let mut grant = grant();
            grant.headroom.headroom = f64::from_bits(0x4000_0000_0000_0000 + offset * 0x12_345);
            let witness: PlacementGrant = rows::decode(&rows::message(&grant).unwrap()).unwrap();
            assert_eq!(
                witness.headroom.headroom.to_bits(),
                grant.headroom.headroom.to_bits()
            );
        }
    }
    #[test]
    fn lifecycle_round_trip_and_refuse_malformed_time() {
        let ledger = ledger();
        let mut record = GrantLifecycle {
            proof: crate::node::GrantCompletion {
                version: 1,
                grant: grant(),
                admission: json!({"request":request(),"result":{"ok":true}}),
            },
            finished_ms: Some(30),
        };
        lifecycles::put(&ledger, "a", &record).unwrap();
        assert_same(&lifecycles::get(&ledger, "a").unwrap().unwrap(), &record);
        record.finished_ms = None;
        lifecycles::put(&ledger, "a", &record).unwrap();
        assert_same(&lifecycles::get(&ledger, "a").unwrap().unwrap(), &record);
        record.finished_ms = Some(u64::MAX);
        assert!(lifecycles::put(&ledger, "bad", &record).is_err());
        assert!(lifecycles::get(&ledger, "bad").unwrap().is_none());
        std::fs::remove_dir_all(ledger.dir).unwrap();
    }
    #[test]
    fn expectation_and_argument_rows_round_trip_and_clear() {
        let ledger = ledger();
        let mut record: PreparationLookup = decode(
            json!({"version":1,"request":request(),"host":{"id":"host","label":"label","target":"target"},"preparation":{"version":2,"validity":{"version":1,"created_ms":10},"request_id":"a","repo":"/repo","branch":"branch","base":"base"},"binding":binding()}),
        );
        expectations::put(&ledger, "a", &record).unwrap();
        assert_same(&expectations::get(&ledger, "a").unwrap().unwrap(), &record);
        record.request.action = crate::work::Action::Place;
        record.binding = None;
        record.preparation.validity = None;
        record.preparation.base = None;
        expectations::put(&ledger, "a", &record).unwrap();
        assert_same(&expectations::get(&ledger, "a").unwrap().unwrap(), &record);
        assert_eq!(
            store::read(&ledger, |tx| Ok(tx.query_row(
                "SELECT count(*) FROM preparation_expectation_args",
                [],
                |row| row.get::<_, i64>(0)
            )?))
            .unwrap(),
            0
        );
        record.preparation.validity = Some(crate::jobs::Validity {
            version: 1,
            created_ms: u64::MAX,
        });
        assert!(expectations::put(&ledger, "bad", &record).is_err());
        assert!(expectations::get(&ledger, "bad").unwrap().is_none());
        std::fs::remove_dir_all(ledger.dir).unwrap();
    }
    #[test]
    fn retirement_and_argument_rows_round_trip_and_clear() {
        let ledger = ledger();
        let disposition = disposition_request();
        let mut record = Intent {
            version: 1,
            request: Some(request()),
            disposition: disposition.clone(),
            acknowledgement: Some(disposition::Acknowledgement {
                version: 1,
                binding: disposition.binding,
                policy: disposition.policy,
                outcome: disposition.outcome,
                released: true,
            }),
            error: Some("error".into()),
        };
        retirements::put(&ledger, "a", &record).unwrap();
        assert_same(&retirements::get(&ledger, "a").unwrap().unwrap(), &record);
        record.request = None;
        record.acknowledgement = None;
        record.error = None;
        retirements::put(&ledger, "a", &record).unwrap();
        assert_same(&retirements::get(&ledger, "a").unwrap().unwrap(), &record);
        assert_eq!(
            store::read(&ledger, |tx| Ok(tx.query_row(
                "SELECT count(*) FROM preparation_retirement_args",
                [],
                |row| row.get::<_, i64>(0)
            )?))
            .unwrap(),
            0
        );
        if let Policy::Prohibit {
            basis: Basis::Retired { audit, .. },
        } = &mut record.disposition.policy
        {
            audit.recorded_ms = u64::MAX;
        }
        assert!(retirements::put(&ledger, "bad", &record).is_err());
        assert!(retirements::get(&ledger, "bad").unwrap().is_none());
        std::fs::remove_dir_all(ledger.dir).unwrap();
    }
}
