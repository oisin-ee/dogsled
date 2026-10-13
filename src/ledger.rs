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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Ledger {
    pub dir: PathBuf,
}

pub fn now_ms() -> u64 {
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
        Self { dir }
    }

    pub fn with_lock<T>(&self, name: &str, f: impl FnOnce() -> Result<T>) -> Result<T> {
        fs::create_dir_all(&self.dir)?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(self.dir.join(format!("{name}.lock")))?;
        lock.lock()?;
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
                    pane: None,
                }],
            )
            .unwrap();
        assert_eq!(ledger.reservations(60_099, 60_000).unwrap().len(), 1);
        assert_eq!(ledger.reservations(60_100, 60_000).unwrap(), Vec::new());
        fs::remove_dir_all(ledger.dir).unwrap();
    }

    #[test]
    fn lock_serializes_concurrent_writers_without_lost_updates() {
        let ledger = Ledger::new(std::env::temp_dir().join(new_id()));
        std::thread::scope(|scope| {
            for _ in 0..8 {
                scope.spawn(|| {
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
