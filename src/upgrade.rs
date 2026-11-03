//! Executable ownership is independent of the application schema. Only command owners
//! exec, at a boundary where their work and resource guards have finished.

use std::path::Path;
use std::sync::OnceLock;

use rusqlite::{Connection, OpenFlags};
use semver::Version;

use crate::error::{Error, Result};
use crate::ledger::Ledger;
use crate::shell::{Shell as _, System};
use crate::tables::Writer;

const FLOOR: &str = "DOGSLED_UPGRADE_TARGET";

#[derive(serde::Serialize, serde::Deserialize)]
struct ExecTarget {
    writer: Writer,
    pid: u32,
}

pub(crate) fn reported_version() -> &'static str {
    static VERSION: OnceLock<String> = OnceLock::new();
    VERSION.get_or_init(|| {
        // Integration tests copy the binary and give only that copy another version.
        // Release binaries never honor this override.
        if cfg!(debug_assertions)
            && let Some(path) = std::env::var_os("DOGSLED_TEST_VERSION_EXE")
            && std::env::current_exe().ok() == std::fs::canonicalize(path).ok()
            && let Ok(version) = std::env::var("DOGSLED_TEST_VERSION")
        {
            return version;
        }
        env!("CARGO_PKG_VERSION").into()
    })
}

fn parsed(version: &str) -> Result<Version> {
    Version::parse(version)
        .map_err(|error| Error::Invalid(format!("invalid dogsled version {version}: {error}")))
}

pub(crate) fn is_newer(writer: &Writer) -> Result<bool> {
    Ok(parsed(&writer.version)?
        .cmp_precedence(&parsed(reported_version())?)
        .is_gt())
}

pub(crate) fn refusal(writer: &Writer, reason: impl std::fmt::Display) -> Error {
    Error::Invalid(format!(
        "state.db writer dogsled {} at {} is newer than this dogsled {}; handover failed: {reason}",
        writer.version,
        writer.executable,
        reported_version()
    ))
}

/// This query deliberately knows nothing about the rest of the database's schema.
pub(crate) fn newer(ledger: &Ledger) -> Result<Option<Writer>> {
    let path = ledger.dir.join("state.db");
    if !path.try_exists()? {
        return Ok(None);
    }
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    connection.busy_timeout(ledger.budget.remaining("state.db writer")?)?;
    match crate::tables::writer(&connection)? {
        Some(writer) if is_newer(&writer).map_err(|error| refusal(&writer, error))? => {
            Ok(Some(writer))
        }
        _ => Ok(None),
    }
}

/// Store transactions refuse a newer owner; they never exec with locks or work live.
pub(crate) fn guard(connection: &Connection) -> Result<()> {
    if let Some(writer) = crate::tables::writer(connection)?
        && is_newer(&writer)?
    {
        return Err(refusal(
            &writer,
            "the command must hand over between units of work",
        ));
    }
    Ok(())
}

pub(crate) fn self_writer() -> Result<Writer> {
    let executable = std::fs::canonicalize(std::env::current_exe()?)?;
    let executable = executable
        .to_str()
        .ok_or_else(|| Error::Invalid("dogsled executable path is not UTF-8".into()))?;
    Ok(Writer {
        version: reported_version().into(),
        executable: executable.into(),
    })
}

pub(crate) fn register(connection: &mut Connection, path: &Path) -> Result<()> {
    use std::os::unix::fs::MetadataExt as _;
    type Registrations = std::sync::Mutex<std::collections::HashSet<(u64, u64)>>;
    static REGISTERED: OnceLock<Registrations> = OnceLock::new();
    let registered = REGISTERED.get_or_init(Registrations::default);
    let metadata = std::fs::metadata(path)?;
    let identity = (metadata.dev(), metadata.ino());
    guard(connection)?;
    let current = self_writer()?;
    let recorded = crate::tables::writer(connection)?;
    // Registration belongs to the process, not each cached/nested connection. An equal
    // version in another installation can record itself without making this process
    // take a second write lock inside one of its own transactions.
    if recorded
        .as_ref()
        .is_some_and(|writer| writer.version == reported_version())
        && registered
            .lock()
            .map_err(|error| Error::Invalid(error.to_string()))?
            .contains(&identity)
    {
        return Ok(());
    }
    // A nested read can open another connection while its caller owns a write
    // transaction. Do not acquire another write lock for an unchanged registration.
    if let Some(writer) = recorded
        && writer.version == reported_version()
        && writer.executable == current.executable
    {
        registered
            .lock()
            .map_err(|error| Error::Invalid(error.to_string()))?
            .insert(identity);
        return Ok(());
    }
    let tx = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    tx.execute_batch(crate::tables::WRITER_SCHEMA)?;
    guard(&tx)?;
    crate::tables::save_writer(&tx, &current)?;
    tx.commit()?;
    registered
        .lock()
        .map_err(|error| Error::Invalid(error.to_string()))?
        .insert(identity);
    Ok(())
}

/// A successful exec must land in the version we validated, never another old image.
pub(crate) fn verify_target() -> Result<()> {
    if let Ok(target) = std::env::var(FLOOR) {
        let target: ExecTarget = serde_json::from_str(&target)?;
        // Child CLI invocations inherit the environment, but not the exec attempt.
        if target.pid == std::process::id() && reported_version() != target.writer.version {
            return Err(refusal(
                &target.writer,
                "the replacement reported a different version after exec",
            ));
        }
    }
    Ok(())
}

/// Validate before leaving the old image. The version probe has a bounded lifetime.
pub(crate) fn validate(writer: &Writer) -> Result<()> {
    let check = || -> Result<()> {
        if !is_newer(writer)? {
            return Err(Error::Invalid("replacement is not strictly newer".into()));
        }
        let path = Path::new(&writer.executable);
        if !path.is_absolute() || std::fs::canonicalize(path)? != path {
            return Err(Error::Invalid(
                "replacement path is not absolute and canonical".into(),
            ));
        }
        let output = System::default().run_bounded(
            &writer.executable,
            &["--version".into()],
            std::time::Duration::from_secs(2),
        )?;
        if !output.ok() || output.stdout.trim() != format!("dogsled {}", writer.version) {
            return Err(Error::Invalid(
                "replacement --version does not match the recorded version".into(),
            ));
        }
        Ok(())
    };
    check().map_err(|error| refusal(writer, error))
}

/// Replace the image, retaining PID, standard streams, environment, and original argv.
/// MCP supplies only its negotiated state and an undispatched input message.
pub(crate) fn handover(writer: &Writer, resume: Option<&str>) -> Result<()> {
    use std::os::unix::process::CommandExt as _;
    validate(writer)?;
    let mut args = std::env::args_os().skip(1).collect::<Vec<_>>();
    // A later upgrade replaces, rather than appends to, the previous resume argument.
    if resume.is_some()
        && let Some(index) = args.iter().position(|arg| arg == "--mcp-resume")
    {
        args.drain(index..index.saturating_add(2).min(args.len()));
    }
    let mut command = std::process::Command::new(&writer.executable);
    command.args(args).env(
        FLOOR,
        serde_json::to_string(&ExecTarget {
            writer: writer.clone(),
            pid: std::process::id(),
        })?,
    );
    if let Some(resume) = resume {
        command.args(["--mcp-resume", resume]);
    }
    let error = command.exec();
    Err(refusal(writer, error))
}

pub(crate) fn check(ledger: &Ledger) -> Result<()> {
    if let Some(writer) = newer(ledger)? {
        handover(&writer, None)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(clippy::unwrap_used)]
    fn an_equal_installation_does_not_deadlock_a_nested_store_read() {
        let mut ledger = Ledger::new(std::env::temp_dir().join(crate::ledger::new_id()));
        ledger.budget = crate::budget::Budget::until(
            std::time::Instant::now() + std::time::Duration::from_secs(2),
        );
        crate::store::write(&ledger, |tx| {
            crate::tables::save_writer(
                tx,
                &Writer {
                    version: reported_version().into(),
                    executable: "/another/equal-install/dogsled".into(),
                },
            )
        })
        .unwrap();
        crate::store::write(&ledger, |_| crate::store::read(&ledger, |_| Ok(()))).unwrap();
        crate::store::release();
        std::fs::remove_dir_all(ledger.dir).unwrap();
    }
}
