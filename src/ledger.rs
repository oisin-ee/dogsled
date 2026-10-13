//! Host-owned snapshots and reservations. Writers use a kernel file lock and
//! atomic replacement; readers never observe a partially written JSON file.

use std::fs::{self, File, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize, de::DeserializeOwned};

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

pub fn now_ms() -> u64 {
    // Debug binaries only: deterministic CLI integration tests. Release clocks
    // cannot be replaced by an environment variable.
    #[cfg(debug_assertions)]
    if let Ok(value) = std::env::var("WRANGLE_TEST_NOW_MS")
        && let Ok(now) = value.parse()
    {
        return now;
    }
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

pub fn default_dir() -> PathBuf {
    if let Some(p) = std::env::var_os("WRANGLE_STATE") {
        return PathBuf::from(p);
    }
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))
        .unwrap_or_else(|| PathBuf::from(".local/state"));
    base.join("wrangle")
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

    pub fn with_lock<T>(&self, name: &str, f: impl FnOnce() -> Result<T>) -> Result<T> {
        fs::create_dir_all(&self.dir)?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(self.dir.join(format!("{name}.lock")))?;
        let operation = format!("lock {}", self.dir.join(format!("{name}.lock")).display());
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
        // Dropping the file releases the lock, including on error or panic.
        f()
    }

    pub fn read<T: DeserializeOwned>(&self, name: &str) -> Result<Option<T>> {
        read(&self.dir.join(name))
    }

    pub fn write<T: Serialize>(&self, name: &str, value: &T) -> Result<()> {
        fs::create_dir_all(&self.dir)?;
        let path = self.dir.join(name);
        let tmp = self.dir.join(format!(".{name}.{}.tmp", new_id()));
        let result = (|| {
            let mut file = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
            serde_json::to_writer_pretty(&mut file, value)?;
            writeln!(file)?;
            file.sync_all()?;
            fs::rename(&tmp, path)?;
            File::open(&self.dir)?.sync_all()?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(tmp);
        }
        result
    }

    pub fn reservations(&self, now: u64, ttl: u64) -> Result<Vec<Reservation>> {
        let all: Vec<Reservation> = self.read("placements.json")?.unwrap_or_default();
        Ok(all
            .into_iter()
            .filter(|r| now.saturating_sub(r.created_ms) < ttl)
            .collect())
    }
}

fn read<T: DeserializeOwned>(path: &Path) -> Result<Option<T>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reservation_expiry_uses_injected_time() {
        let ledger = Ledger::new(std::env::temp_dir().join(new_id()));
        ledger
            .write(
                "placements.json",
                &vec![Reservation {
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
        std::thread::scope(|scope| {
            for _ in 0..8 {
                scope.spawn(|| {
                    barrier.wait();
                    ledger
                        .with_lock("host", || {
                            let n: u64 = ledger.read("counter.json")?.unwrap_or_default();
                            ledger.write("counter.json", &(n + 1))
                        })
                        .unwrap();
                });
            }
        });
        assert_eq!(ledger.read::<u64>("counter.json").unwrap(), Some(8));
        fs::remove_dir_all(ledger.dir).unwrap();
    }
}
