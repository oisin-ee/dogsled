//! The host state directory, its clock and ids, and kernel locks for outside resources.
//! Records live in `state.db`; see `store`.

use std::fs::{self, File, OpenOptions};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::error::Result;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reservation {
    pub id: String,
    pub created_ms: u64,
}

#[derive(Debug, Clone)]
pub struct Ledger {
    pub dir: PathBuf,
    pub budget: crate::budget::Budget,
}

/// An owned kernel lock. Explicit unlock also releases copies inherited at fork.
#[derive(Debug)]
pub struct Lock(File);

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

pub fn now_ms() -> u64 {
    // Debug binaries only: deterministic CLI integration tests. Release clocks
    // cannot be replaced by an environment variable.
    #[cfg(debug_assertions)]
    if let Ok(value) = std::env::var("DOGSLED_TEST_NOW_MS")
        && let Ok(now) = value.parse()
    {
        return now;
    }
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

pub fn default_dir() -> PathBuf {
    if let Some(p) = std::env::var_os("DOGSLED_STATE") {
        return PathBuf::from(p);
    }
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))
        .unwrap_or_else(|| PathBuf::from(".local/state"));
    base.join("dogsled")
}

pub fn new_id() -> String {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    format!(
        "r-{}-{}-{}",
        now_ms(),
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    )
}

impl Ledger {
    pub fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            budget: crate::budget::Budget::default(),
        }
    }

    pub fn child(&self, name: &str) -> Self {
        Self {
            dir: self.dir.join(name),
            budget: self.budget,
        }
    }

    pub(crate) fn lock_path(&self, name: &str) -> PathBuf {
        self.dir.join(format!("{name}.lock"))
    }

    fn lock_file(&self, name: &str) -> Result<File> {
        fs::create_dir_all(&self.dir)?;
        Ok(OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(self.lock_path(name))?)
    }

    /// Acquire a lifetime lock without waiting for another owner.
    pub fn try_lock(&self, name: &str) -> Result<Option<Lock>> {
        let lock = self.lock_file(name)?;
        match lock.try_lock() {
            Ok(()) => Ok(Some(Lock(lock))),
            Err(std::fs::TryLockError::WouldBlock) => Ok(None),
            Err(std::fs::TryLockError::Error(error)) => Err(error.into()),
        }
    }

    pub fn with_lock<T>(&self, name: &str, f: impl FnOnce() -> Result<T>) -> Result<T> {
        let lock = self.lock_file(name)?;
        let operation = format!("lock {}", self.lock_path(name).display());
        let end = std::time::Instant::now() + self.budget.remaining(&operation)?;
        loop {
            match lock.try_lock() {
                Ok(()) => break,
                Err(std::fs::TryLockError::WouldBlock) => {
                    let remaining = end.saturating_duration_since(std::time::Instant::now());
                    if remaining.is_zero() {
                        return Err(crate::error::Error::Deadline(operation));
                    }
                    std::thread::sleep(remaining.min(std::time::Duration::from_millis(2)));
                }
                Err(std::fs::TryLockError::Error(error)) => return Err(error.into()),
            }
        }
        let lock = Lock(lock);
        let result = f();
        // Preserve unlock errors on ordinary completion; Drop also unlocks on unwind.
        lock.0.unlock()?;
        result
    }

    pub fn reservations(&self, now: u64, ttl: u64) -> Result<Vec<Reservation>> {
        crate::job_store::live_reservations(self, now, ttl)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reservation_expiry_uses_injected_time() {
        let ledger = Ledger::new(std::env::temp_dir().join(new_id()));
        crate::job_store::put_reservations(
            &ledger,
            &[Reservation {
                id: "one".into(),
                created_ms: 100,
            }],
        )
        .unwrap();
        let ttl = crate::config::Policy::default().reservation_ttl_ms;
        assert_eq!(ttl, 90_000);
        assert_eq!(ledger.reservations(90_099, ttl).unwrap().len(), 1);
        assert_eq!(ledger.reservations(90_100, ttl).unwrap(), Vec::new());
        fs::remove_dir_all(ledger.dir).unwrap();
    }

    #[test]
    fn lock_serializes_concurrent_writers_without_lost_updates() {
        let ledger = Ledger::new(std::env::temp_dir().join(new_id()));
        let barrier = std::sync::Barrier::new(8);
        let counter = AtomicU64::new(0);
        std::thread::scope(|scope| {
            for _ in 0..8 {
                scope.spawn(|| {
                    barrier.wait();
                    ledger
                        .with_lock("host", || {
                            let n = counter.load(Ordering::Relaxed);
                            std::thread::yield_now();
                            counter.store(n + 1, Ordering::Relaxed);
                            Ok(())
                        })
                        .unwrap();
                });
            }
        });
        assert_eq!(counter.load(Ordering::Relaxed), 8);
        fs::remove_dir_all(ledger.dir).unwrap();
    }
}
