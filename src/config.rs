//! Host policy. Worktree setup and teardown belong to each repository's `dogsled.toml`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// Default way to run `dogsled` on a remote host.
pub const DEFAULT_REMOTE_COMMAND: &[&str] = &["mise", "x", "--", "dogsled"];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Policy {
    /// Admit while free cores stay at or above this fraction of the host's cores.
    /// Free cores are the smaller of measured (`cores × (1 − busy) − reservations`)
    /// and declared (`cores − active agents − reservations`), each scaled by
    /// `reservation_load`. 0.25 scored the lowest
    /// admission error on both sampled hosts (4 of 16, 3 of 12 cores).
    pub free_core_fraction_min: f64,
    /// Bytes that must be free for the new agent and for each unexpired
    /// reservation, and that the host's total must hold for each active agent
    /// besides: agent p90 RSS (0.4 GiB) plus build-tree p90 RSS
    /// (2.5–3.2 GiB) measured on the fleet.
    pub memory_reserve_bytes: u64,
    /// Admit while the home filesystem keeps at least this much free.
    pub disk_free_min_percent: f64,
    /// Cores each active agent and each unexpired reservation subtracts from the
    /// free cores. One agent's mean demand is 0.5–0.9 cores (build duty cycle × median build
    /// tree), so one core each.
    pub reservation_load: f64,
    /// A reservation older than this is dropped on the next probe. It covers
    /// the time until the new agent shows in `herdr agent list` and its first
    /// build shows in the busy fraction.
    pub reservation_ttl_ms: u64,
    /// argv prefix that reaches `dogsled` on a remote host over
    /// `ssh -T -o BatchMode=yes`. mise shims are not on the non-interactive
    /// PATH, but `mise` itself is, so the default goes through `mise x`.
    pub remote_command: Vec<String>,
    pub cleanup: Cleanup,
    pub quota: Quota,
    pub web: Web,
}

/// The web portal's settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Web {
    /// The port `dogsled web` listens on. The server binds the loopback address.
    pub port: u16,
}

impl Default for Web {
    fn default() -> Self {
        Self { port: 7465 }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Quota {
    /// Non-secret `CLIProxy` origin. The management key belongs only in macOS Keychain.
    pub management_url: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Cleanup {
    /// Repositories doctor checks besides the registered ones, the Herdr workspaces, and the
    /// caller's. A leading `~/` is the home directory.
    pub repositories: Vec<String>,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            free_core_fraction_min: 0.25,
            memory_reserve_bytes: 3 * 1024 * 1024 * 1024,
            disk_free_min_percent: 15.0,
            reservation_load: 1.0,
            reservation_ttl_ms: 90_000,
            remote_command: DEFAULT_REMOTE_COMMAND
                .iter()
                .map(ToString::to_string)
                .collect(),
            cleanup: Cleanup::default(),
            quota: Quota::default(),
            web: Web::default(),
        }
    }
}

/// The subset of the policy a remote host needs to re-check admission under
/// its own lock. Serialised on the command line of `dogsled host reserve`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Thresholds {
    pub free_core_fraction_min: f64,
    pub memory_reserve_bytes: u64,
    pub disk_free_min_percent: f64,
    pub reservation_load: f64,
    pub reservation_ttl_ms: u64,
}

impl From<&Policy> for Thresholds {
    fn from(p: &Policy) -> Self {
        Self {
            free_core_fraction_min: p.free_core_fraction_min,
            memory_reserve_bytes: p.memory_reserve_bytes,
            disk_free_min_percent: p.disk_free_min_percent,
            reservation_load: p.reservation_load,
            reservation_ttl_ms: p.reservation_ttl_ms,
        }
    }
}

/// `$DOGSLED_CONFIG`, else `$XDG_CONFIG_HOME/dogsled/config.toml`, else
/// `~/.config/dogsled/config.toml`.
#[must_use]
pub fn default_path() -> PathBuf {
    if let Some(p) = std::env::var_os("DOGSLED_CONFIG") {
        return PathBuf::from(p);
    }
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from(".config"));
    base.join("dogsled").join("config.toml")
}

/// A missing file means defaults. A present but invalid file is an error, so a
/// typo never silently reverts to the defaults.
pub fn load(path: &Path) -> Result<Policy> {
    match std::fs::read_to_string(path) {
        Ok(text) => parse(&text).map_err(|source| Error::Config {
            path: path.display().to_string(),
            source,
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Policy::default()),
        Err(e) => Err(e.into()),
    }
}

pub fn parse(text: &str) -> std::result::Result<Policy, toml::de::Error> {
    toml::from_str(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_text_is_defaults() {
        assert_eq!(parse("").unwrap(), Policy::default());
    }

    #[test]
    fn partial_override_keeps_other_defaults() {
        let p = parse("free_core_fraction_min = 0.5\n").unwrap();
        assert!((p.free_core_fraction_min - 0.5).abs() < f64::EPSILON);
        assert!((p.disk_free_min_percent - 15.0).abs() < f64::EPSILON);
        assert_eq!(p.memory_reserve_bytes, 3 * 1024 * 1024 * 1024);
    }

    #[test]
    fn unknown_key_is_rejected() {
        assert!(parse("max_agents = 3\n").is_err());
        // Worktree setup and teardown moved to the repository's `dogsled.toml`.
        assert!(parse("[hooks]\nprepare = [\"true\"]\n").is_err());
    }

    #[test]
    fn missing_file_is_defaults() {
        let p = load(Path::new("/nonexistent/dogsled/config.toml")).unwrap();
        assert_eq!(p, Policy::default());
    }

    #[test]
    fn the_web_port_has_a_default_and_can_be_set() {
        assert_eq!(Policy::default().web.port, 7465);
        let p = parse("[web]\nport = 8080\n").unwrap();
        assert_eq!(p.web.port, 8080);
        // The rest of the policy keeps its defaults.
        assert_eq!(p.disk_free_min_percent, 15.0);
    }

    #[test]
    fn an_unknown_web_key_is_rejected() {
        assert!(parse("[web]\nports = 8080\n").is_err());
    }
}
