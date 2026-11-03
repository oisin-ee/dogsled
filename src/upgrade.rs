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
            && let Ok(current) = std::env::current_exe().and_then(std::fs::canonicalize)
            && let Ok(path) = std::fs::canonicalize(path)
            && current == path
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
    Error::NewerWriter(format!(
        "state.db writer dogsled {} at {} is newer than this dogsled {}; handover failed: {reason}",
        writer.version,
        writer.executable,
        reported_version()
    ))
}

pub(crate) enum Boundary {
    Current,
    Ready(Writer),
    Pending(Error),
}

#[derive(Clone, Copy)]
enum Validation {
    Valid,
    Stale,
}

type Validations = std::sync::Mutex<std::collections::HashMap<(String, String), Validation>>;
static VALIDATED: OnceLock<Validations> = OnceLock::new();

/// This query deliberately knows nothing about the rest of the database's schema.
pub(crate) fn newer(ledger: &Ledger) -> Result<Boundary> {
    match boundary(ledger) {
        // A writer can change while a stale registration is being replaced. Its
        // transaction guard has the same retry semantics as an inconclusive probe.
        Err(error) if matches!(error.root(), Error::NewerWriter(_)) => Ok(Boundary::Pending(error)),
        result => result,
    }
}

fn boundary(ledger: &Ledger) -> Result<Boundary> {
    let path = ledger.dir.join("state.db");
    if !path.try_exists()? {
        return Ok(Boundary::Current);
    }
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    connection.busy_timeout(ledger.budget.remaining("state.db writer")?)?;
    match crate::tables::writer(&connection)? {
        Some(writer) if is_newer(&writer).map_err(|error| refusal(&writer, error))? => {
            match validated(&writer) {
                Ok(Validation::Valid) => return Ok(Boundary::Ready(writer)),
                Ok(Validation::Stale) => {}
                Err(error) => return Ok(Boundary::Pending(error)),
            }
            crate::store::schema_version(&connection)?;
            drop(connection);
            // Opening the store replaces a stale owner under its write transaction.
            crate::store::write(ledger, |_| Ok(()))?;
            Ok(Boundary::Current)
        }
        _ => Ok(Boundary::Current),
    }
}

/// Store transactions refuse a newer owner; they never exec with locks or work live.
pub(crate) fn guard(connection: &Connection) -> Result<()> {
    // Ownership must be classified even if the newer writer also changed schema.
    // A running owner retries at its boundary; stale writers still cannot open it.
    if let Some(writer) = crate::tables::writer(connection)?
        && is_newer(&writer)?
        && matches!(validated(&writer)?, Validation::Valid)
    {
        return Err(refusal(
            &writer,
            "the command must hand over between units of work",
        ));
    }
    crate::store::schema_version(connection)?;
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

/// Explicit state directories remain available to debug builds and test ledgers.
pub(crate) fn registration_allowed(path: &Path) -> Result<bool> {
    Ok(!cfg!(debug_assertions)
        || std::env::var_os("DOGSLED_STATE").is_some()
        || path.parent() != Some(std::path::absolute(crate::ledger::default_dir())?.as_path()))
}

pub(crate) fn register(connection: &mut Connection, path: &Path) -> Result<()> {
    use std::os::unix::fs::MetadataExt as _;
    type Registrations = std::sync::Mutex<std::collections::HashSet<(u64, u64)>>;
    static REGISTERED: OnceLock<Registrations> = OnceLock::new();
    if !registration_allowed(path)? {
        return Ok(());
    }
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
    let stale = crate::tables::writer(&tx)?.filter(|writer| writer.version != current.version);
    crate::tables::save_writer(&tx, &current)?;
    tx.commit()?;
    if let Some(writer) = stale
        && is_newer(&writer)?
    {
        eprintln!(
            "dogsled: replaced stale state.db writer dogsled {} at {} with dogsled {} at {}",
            writer.version, writer.executable, current.version, current.executable
        );
    }
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

/// A timeout or unsuccessful launch cannot prove that the recorded writer is stale.
fn probe(writer: &Writer) -> Result<Validation> {
    use std::os::unix::fs::PermissionsExt as _;
    let path = Path::new(&writer.executable);
    let canonical = match std::fs::canonicalize(path) {
        Ok(path) => path,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Validation::Stale);
        }
        Err(error) => return Err(error.into()),
    };
    if !path.is_absolute() || canonical != path {
        return Ok(Validation::Stale);
    }
    let metadata = std::fs::metadata(path)?;
    if !metadata.is_file() || metadata.permissions().mode() & 0o111 == 0 {
        return Ok(Validation::Stale);
    }
    let output = match System::default().run_bounded(
        &writer.executable,
        &["--version".into()],
        std::time::Duration::from_secs(10),
    ) {
        Err(Error::Io(error))
            if error.kind() == std::io::ErrorKind::PermissionDenied
                || error.raw_os_error() == Some(nix::errno::Errno::ENOEXEC as i32) =>
        {
            return Ok(Validation::Stale);
        }
        output => output?,
    };
    if !output.ok() {
        return Err(Error::Invalid(
            "replacement --version did not exit successfully; ownership is unverified".into(),
        ));
    }
    Ok(
        if output.stdout.trim() == format!("dogsled {}", writer.version) {
            Validation::Valid
        } else {
            Validation::Stale
        },
    )
}

/// Cache definite outcomes once per executable/version pair. Retry inconclusive probes.
fn validated(writer: &Writer) -> Result<Validation> {
    let mut validations = VALIDATED
        .get_or_init(Validations::default)
        .lock()
        .map_err(|error| Error::Invalid(error.to_string()))?;
    let key = (writer.executable.clone(), writer.version.clone());
    if let Some(validation) = validations.get(&key) {
        return Ok(*validation);
    }
    let validation = probe(writer).map_err(|error| refusal(writer, error))?;
    validations.insert(key, validation);
    Ok(validation)
}

/// Replace the image, retaining PID, standard streams, environment, and original argv.
/// MCP supplies only its negotiated state and an undispatched input message.
pub(crate) fn handover(writer: &Writer, resume: Option<&str>) -> Result<()> {
    use std::os::unix::process::CommandExt as _;
    if !is_newer(writer)? || matches!(validated(writer)?, Validation::Stale) {
        return Err(refusal(
            writer,
            "replacement is not a valid newer executable",
        ));
    }
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
    // The file can change after validation. A failed exec invalidates that proof;
    // the next owner boundary must probe again instead of reusing a valid target.
    VALIDATED
        .get_or_init(Validations::default)
        .lock()
        .map_err(|error| Error::Invalid(error.to_string()))?
        .remove(&(writer.executable.clone(), writer.version.clone()));
    Err(refusal(writer, error))
}

pub(crate) fn check(ledger: &Ledger) -> Result<()> {
    match newer(ledger)? {
        Boundary::Current => Ok(()),
        Boundary::Ready(writer) => handover(&writer, None),
        Boundary::Pending(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(clippy::unwrap_used)]
    fn transaction_guards_cache_success_and_failure_per_writer_pair() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = std::env::temp_dir().join(crate::ledger::new_id());
        std::fs::create_dir_all(&dir).unwrap();
        let dir = std::fs::canonicalize(dir).unwrap();
        let executable = dir.join("probe");
        let probes = dir.join("probes");
        std::fs::write(
            &executable,
            format!(
                "#!/bin/sh\necho probe >> '{}'\necho 'dogsled 999.0.0'\n",
                probes.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(crate::tables::WRITER_SCHEMA)
            .unwrap();
        for (version, valid) in [("999.0.0", true), ("999.0.1", false)] {
            let tx = connection.transaction().unwrap();
            crate::tables::save_writer(
                &tx,
                &Writer {
                    executable: executable.to_str().unwrap().into(),
                    version: version.into(),
                },
            )
            .unwrap();
            tx.commit().unwrap();
            for _ in 0..3 {
                assert_eq!(guard(&connection).is_err(), valid);
            }
        }
        assert_eq!(std::fs::read_to_string(probes).unwrap().lines().count(), 2);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    #[allow(clippy::unwrap_used)]
    fn inconclusive_probes_keep_the_guard_and_are_not_cached() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = std::env::temp_dir().join(crate::ledger::new_id());
        std::fs::create_dir_all(&dir).unwrap();
        let dir = std::fs::canonicalize(dir).unwrap();
        let executable = dir.join("probe");
        let probes = dir.join("probes");
        let writer = Writer {
            executable: executable.to_str().unwrap().into(),
            version: "999.0.0".into(),
        };
        let mut connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(crate::tables::WRITER_SCHEMA)
            .unwrap();
        let tx = connection.transaction().unwrap();
        crate::tables::save_writer(&tx, &writer).unwrap();
        tx.commit().unwrap();
        // A schema change inside the pass must not hide the newer-writer refusal.
        connection
            .pragma_update(None, "user_version", i32::MAX)
            .unwrap();
        for (body, expected) in [
            ("exec sleep 60", "deadline reached"),
            ("exit 1", "did not exit successfully"),
            ("echo 'dogsled 999.0.0'", "command must hand over"),
        ] {
            std::fs::write(
                &executable,
                format!("#!/bin/sh\necho probe >> '{}'\n{body}\n", probes.display()),
            )
            .unwrap();
            std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
            let error = guard(&connection).unwrap_err();
            assert!(matches!(error.root(), Error::NewerWriter(_)), "{error}");
            assert!(error.to_string().contains(expected), "{error}");
            let recorded = crate::tables::writer(&connection).unwrap().unwrap();
            assert_eq!(recorded.version, writer.version);
            assert_eq!(recorded.executable, writer.executable);
        }
        assert!(matches!(validated(&writer).unwrap(), Validation::Valid));
        assert_eq!(std::fs::read_to_string(probes).unwrap().lines().count(), 3);
        std::fs::remove_file(executable).unwrap();
        let error = handover(&writer, None).unwrap_err();
        assert!(matches!(error.root(), Error::NewerWriter(_)), "{error}");
        assert!(matches!(validated(&writer).unwrap(), Validation::Stale));
        std::fs::remove_dir_all(dir).unwrap();
    }

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
