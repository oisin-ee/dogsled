//! Typed row helpers shared by the `state.db` tables: column values, the request and
//! diagnostic column groups, and exact protocol message bytes.

use rusqlite::types::Value as Sql;
use rusqlite::{Row, Transaction};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::error::{Error, Result};
use crate::work::{Action, Request};

/// The columns of a [`Request`]. Job arguments live in a child table of the owner.
macro_rules! request_sql {
    () => {
        "
    request_id TEXT NOT NULL,
    request_validity_version INTEGER,
    request_validity_created_ms INTEGER,
    action TEXT NOT NULL CHECK (action IN ('place', 'job', 'agent')),
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
    request_cwd TEXT NOT NULL,
    repository_path TEXT,
    repository_origin TEXT,
    repository_relative TEXT,
    repository_working_root TEXT,
    request_workspace TEXT,
    request_workspace_name TEXT,
    request_owner TEXT NOT NULL,
    request_owner_host TEXT,"
    };
}

/// The columns of a [`crate::diagnostic::Diagnostic`].
macro_rules! diagnostic_sql {
    () => {
        "
    diagnostic_version INTEGER,
    diagnostic_operation TEXT,
    diagnostic_cause TEXT,
    diagnostic_branch TEXT,
    diagnostic_base TEXT,
    diagnostic_workspace TEXT,
    diagnostic_occupancy TEXT,
    diagnostic_new_worktree_removed INTEGER,
    diagnostic_status INTEGER,
    diagnostic_host TEXT,
    diagnostic_request_id TEXT,
    diagnostic_omission TEXT,
    diagnostic_directory TEXT,"
    };
}

pub(crate) use {diagnostic_sql, request_sql};

/// Named column values for one row. A column that is not set is stored as NULL.
#[derive(Default)]
pub(crate) struct Columns(Vec<(&'static str, Sql)>);

impl Columns {
    pub(crate) fn put(&mut self, column: &'static str, value: Sql) {
        self.0.push((column, value));
    }
    pub(crate) fn text(&mut self, column: &'static str, value: Option<&str>) {
        self.put(
            column,
            value.map_or(Sql::Null, |value| Sql::Text(value.to_owned())),
        );
    }
    pub(crate) fn number(&mut self, column: &'static str, value: Option<u64>) -> Result<()> {
        let value = value.map(integer).transpose()?;
        self.put(column, value.map_or(Sql::Null, Sql::Integer));
        Ok(())
    }
    pub(crate) fn flag(&mut self, column: &'static str, value: bool) {
        self.put(column, Sql::Integer(i64::from(value)));
    }
    /// The exact bytes of a protocol message, or NULL.
    pub(crate) fn message<T: Serialize>(
        &mut self,
        column: &'static str,
        value: Option<&T>,
    ) -> Result<()> {
        let value = optional(value, message)?;
        self.put(column, value.map_or(Sql::Null, Sql::Blob));
        Ok(())
    }

    /// Insert or update the row of `table` keyed by `key`. Every column is written, so an
    /// update clears what the record no longer holds; other columns keep their values.
    pub(crate) fn upsert(&self, tx: &Transaction<'_>, table: &str, key: &str) -> Result<()> {
        let statement = tx.prepare_cached(&format!("SELECT * FROM {table} LIMIT 0"))?;
        let all: Vec<String> = statement
            .column_names()
            .map(str::to_owned)
            .filter(|column| column != "seq")
            .collect();
        drop(statement);
        let values: Vec<Sql> = all
            .iter()
            .map(|column| {
                self.0
                    .iter()
                    .find(|(name, _)| name == column)
                    .map_or(Sql::Null, |(_, value)| value.clone())
            })
            .collect();
        let placeholders = (1..=all.len())
            .map(|n| format!("?{n}"))
            .collect::<Vec<_>>()
            .join(", ");
        let updates = all
            .iter()
            .filter(|column| *column != key)
            .map(|column| format!("{column} = excluded.{column}"))
            .collect::<Vec<_>>()
            .join(", ");
        tx.prepare_cached(&format!(
            "INSERT INTO {table} ({}) VALUES ({placeholders}) ON CONFLICT ({key}) DO UPDATE SET {updates}",
            all.join(", ")
        ))?
        .execute(rusqlite::params_from_iter(values))?;
        Ok(())
    }
}

pub(crate) fn integer(value: u64) -> Result<i64> {
    i64::try_from(value).map_err(|_| Error::Invalid(format!("{value} exceeds a stored integer")))
}

pub(crate) fn unsigned(value: i64) -> Result<u64> {
    u64::try_from(value).map_err(|_| Error::Invalid(format!("stored integer {value} is negative")))
}

/// The serialized name of a unit enum variant.
pub(crate) fn name<T: Serialize>(value: &T) -> Result<String> {
    match serde_json::to_value(value)? {
        serde_json::Value::String(name) => Ok(name),
        other => Err(Error::Invalid(format!("{other} is not a variant name"))),
    }
}

pub(crate) fn variant<T: DeserializeOwned>(name: String) -> Result<T> {
    Ok(serde_json::from_value(serde_json::Value::String(name))?)
}

/// The exact bytes a protocol message is exchanged as.
pub(crate) fn message<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    Ok(serde_json::to_vec(value)?)
}

pub(crate) fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    Ok(serde_json::from_slice(bytes)?)
}

/// A protocol message column, decoded from its exact bytes.
pub(crate) fn decoded<T: DeserializeOwned>(row: &Row<'_>, column: &str) -> Result<Option<T>> {
    optional(bytes(row, column)?, |message| decode(&message))
}

pub(crate) fn optional<T, U>(
    value: Option<T>,
    f: impl FnOnce(T) -> Result<U>,
) -> Result<Option<U>> {
    value.map(f).transpose()
}

/// The rows of a child table for one owner, in order.
pub(crate) fn list<T>(
    tx: &Transaction<'_>,
    sql: &str,
    id: &str,
    f: impl Fn(&Row<'_>) -> rusqlite::Result<T>,
) -> Result<Vec<T>> {
    let mut statement = tx.prepare_cached(sql)?;
    let rows = statement.query_map([id], f)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Replace the ordered values of a child table for one owner.
pub(crate) fn replace_list(
    tx: &Transaction<'_>,
    table: &str,
    owner: &str,
    column: &str,
    id: &str,
    values: impl IntoIterator<Item = Sql>,
) -> Result<()> {
    tx.prepare_cached(&format!("DELETE FROM {table} WHERE {owner} = ?1"))?
        .execute([id])?;
    let mut insert = tx.prepare_cached(&format!(
        "INSERT INTO {table} ({owner}, position, {column}) VALUES (?1, ?2, ?3)"
    ))?;
    for (position, value) in (0_i64..).zip(values) {
        insert.execute(rusqlite::params![id, position, value])?;
    }
    Ok(())
}

pub(crate) fn validity(
    version: Option<i64>,
    created_ms: Option<i64>,
) -> Result<Option<crate::jobs::Validity>> {
    match (version, created_ms) {
        (Some(version), Some(created_ms)) => Ok(Some(crate::jobs::Validity {
            version: u32::try_from(version)
                .map_err(|_| Error::Invalid(format!("validity version {version}")))?,
            created_ms: unsigned(created_ms)?,
        })),
        (None, None) => Ok(None),
        _ => Err(Error::Invalid("validity is incomplete".into())),
    }
}

/// Set the request columns, and return the job arguments for the owner's child table.
pub(crate) fn request_columns(columns: &mut Columns, request: &Request) -> Result<Vec<String>> {
    columns.text("request_id", Some(&request.id));
    columns.number(
        "request_validity_version",
        request.validity.map(|v| u64::from(v.version)),
    )?;
    columns.number(
        "request_validity_created_ms",
        request.validity.map(|v| v.created_ms),
    )?;
    let mut args = Vec::new();
    match &request.action {
        Action::Place => columns.text("action", Some("place")),
        Action::Job {
            cmd,
            timeout_ms,
            idle_timeout_ms,
            validity,
        } => {
            columns.text("action", Some("job"));
            columns.number("job_timeout_ms", *timeout_ms)?;
            columns.number("job_idle_timeout_ms", *idle_timeout_ms)?;
            columns.number(
                "job_validity_version",
                validity.map(|v| u64::from(v.version)),
            )?;
            columns.number("job_validity_created_ms", validity.map(|v| v.created_ms))?;
            args.clone_from(cmd);
        }
        Action::Agent {
            kind,
            name,
            message,
            profile,
        } => {
            columns.text("action", Some("agent"));
            columns.text("agent_kind", Some(kind));
            columns.text("agent_name", Some(name));
            columns.text("agent_message", Some(message));
            columns.text("agent_profile", profile.as_deref());
        }
    }
    columns.text("request_host", request.host.as_deref());
    columns.text("request_branch", request.branch.as_deref());
    columns.text("request_base", request.base.as_deref());
    columns.text("request_cwd", Some(&request.cwd));
    if let Some(repository) = &request.repository {
        columns.text("repository_path", Some(&repository.path));
        columns.text("repository_origin", Some(&repository.origin));
        columns.text("repository_relative", Some(&repository.relative));
        columns.text("repository_working_root", Some(&repository.working_root));
    }
    columns.text("request_workspace", request.workspace.as_deref());
    columns.text("request_workspace_name", request.workspace_name.as_deref());
    columns.text("request_owner", Some(&request.owner));
    columns.text("request_owner_host", request.owner_host.as_deref());
    Ok(args)
}

pub(crate) fn diagnostic_columns(
    columns: &mut Columns,
    diagnostic: Option<&crate::diagnostic::Diagnostic>,
) -> Result<()> {
    let Some(diagnostic) = diagnostic else {
        return Ok(());
    };
    columns.put(
        "diagnostic_version",
        Sql::Integer(i64::from(diagnostic.version)),
    );
    columns.put(
        "diagnostic_operation",
        Sql::Text(name(&diagnostic.operation)?),
    );
    columns.put("diagnostic_cause", Sql::Text(name(&diagnostic.cause)?));
    columns.text("diagnostic_branch", diagnostic.context.branch.as_deref());
    columns.text("diagnostic_base", diagnostic.context.base.as_deref());
    columns.text(
        "diagnostic_directory",
        diagnostic.context.directory.as_deref(),
    );
    columns.text(
        "diagnostic_workspace",
        diagnostic.context.workspace.as_deref(),
    );
    columns.put(
        "diagnostic_occupancy",
        optional(diagnostic.context.occupancy.as_ref(), name)?.map_or(Sql::Null, Sql::Text),
    );
    columns.flag(
        "diagnostic_new_worktree_removed",
        diagnostic.context.new_worktree_removed,
    );
    columns.put(
        "diagnostic_status",
        diagnostic
            .status
            .map_or(Sql::Null, |s| Sql::Integer(i64::from(s))),
    );
    columns.text("diagnostic_host", diagnostic.host.as_deref());
    columns.text("diagnostic_request_id", diagnostic.request_id.as_deref());
    columns.put(
        "diagnostic_omission",
        Sql::Text(name(&diagnostic.omission)?),
    );
    Ok(())
}

pub(crate) fn text(row: &Row<'_>, column: &str) -> Result<Option<String>> {
    Ok(row.get(column)?)
}

pub(crate) fn required(row: &Row<'_>, column: &str) -> Result<String> {
    Ok(row.get(column)?)
}

pub(crate) fn number(row: &Row<'_>, column: &str) -> Result<Option<u64>> {
    row.get::<_, Option<i64>>(column)?.map(unsigned).transpose()
}

pub(crate) fn bytes(row: &Row<'_>, column: &str) -> Result<Option<Vec<u8>>> {
    Ok(row.get(column)?)
}

/// The request stored in `row`, with `args` read from the owner's job-argument table.
pub(crate) fn load_request(
    row: &Row<'_>,
    args: impl FnOnce() -> Result<Vec<String>>,
) -> Result<Request> {
    let validity_of = |version: &str, created: &str| validity(row.get(version)?, row.get(created)?);
    let action = match required(row, "action")?.as_str() {
        "place" => Action::Place,
        "job" => Action::Job {
            cmd: args()?,
            timeout_ms: number(row, "job_timeout_ms")?,
            idle_timeout_ms: number(row, "job_idle_timeout_ms")?,
            validity: validity_of("job_validity_version", "job_validity_created_ms")?,
        },
        _ => Action::Agent {
            kind: required(row, "agent_kind")?,
            name: required(row, "agent_name")?,
            message: required(row, "agent_message")?,
            profile: text(row, "agent_profile")?,
        },
    };
    let repository = match text(row, "repository_path")? {
        Some(path) => Some(crate::checkout::Repository {
            path,
            origin: required(row, "repository_origin")?,
            relative: required(row, "repository_relative")?,
            working_root: required(row, "repository_working_root")?,
        }),
        None => None,
    };
    Ok(Request {
        id: required(row, "request_id")?,
        validity: validity_of("request_validity_version", "request_validity_created_ms")?,
        action,
        host: text(row, "request_host")?,
        branch: text(row, "request_branch")?,
        base: text(row, "request_base")?,
        cwd: required(row, "request_cwd")?,
        repository,
        workspace: text(row, "request_workspace")?,
        workspace_name: text(row, "request_workspace_name")?,
        owner: required(row, "request_owner")?,
        owner_host: text(row, "request_owner_host")?,
    })
}

pub(crate) fn load_diagnostic(row: &Row<'_>) -> Result<Option<crate::diagnostic::Diagnostic>> {
    let Some(version) = row.get::<_, Option<i64>>("diagnostic_version")? else {
        return Ok(None);
    };
    Ok(Some(crate::diagnostic::Diagnostic {
        version: u8::try_from(version)
            .map_err(|_| Error::Invalid(format!("diagnostic version {version}")))?,
        operation: variant(required(row, "diagnostic_operation")?)?,
        cause: variant(required(row, "diagnostic_cause")?)?,
        context: crate::diagnostic::Context {
            branch: text(row, "diagnostic_branch")?,
            base: text(row, "diagnostic_base")?,
            workspace: text(row, "diagnostic_workspace")?,
            directory: text(row, "diagnostic_directory")?,
            occupancy: optional(text(row, "diagnostic_occupancy")?, variant)?,
            new_worktree_removed: number(row, "diagnostic_new_worktree_removed")? == Some(1),
        },
        status: row
            .get::<_, Option<i64>>("diagnostic_status")?
            .map(|s| i32::try_from(s).map_err(|_| Error::Invalid(format!("status {s}"))))
            .transpose()?,
        host: text(row, "diagnostic_host")?,
        request_id: text(row, "diagnostic_request_id")?,
        omission: variant(required(row, "diagnostic_omission")?)?,
    }))
}
