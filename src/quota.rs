//! The shared quota contract and its sole credential-bearing collector.
//! Credentials and response bodies never enter diagnostics or persistent state.

use std::fmt::Write as _;
use std::io::Read as _;

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};

use crate::backoff::Backoff;
use crate::config::Policy;
use crate::error::{Error, Result};
use crate::hosts::Fleet;
use crate::ledger::Ledger;
use crate::shell::Shell;

const CACHE: &str = "quota.json";
const REFRESH_MS: u64 = 5 * 60_000;
const MAX_SNAPSHOT: u64 = 1024 * 1024;
const KEY_ARGS: &[&str] = &[
    "find-generic-password",
    "-s",
    "dogsled",
    "-a",
    "cliproxy-management",
    "-w",
];
const CURL_ARGS: &[&str] = &["-q", "--config", "-"];

type Collected<T> = std::result::Result<T, String>;

/// Error rows for a plugin credential use this window: its real windows are unknown.
const PLUGIN_ERROR_WINDOW: &str = "quota";

/// Providers dogsled reads through `CLIProxy` `api-call`, with fixed windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pool {
    Claude,
    Codex,
}

impl Pool {
    fn parse(name: &str) -> Option<Self> {
        match name {
            "claude" => Some(Self::Claude),
            "codex" => Some(Self::Codex),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }

    fn windows(self) -> &'static [&'static str] {
        match self {
            Self::Claude => &[
                "five_hour",
                "seven_day",
                "seven_day_opus",
                "seven_day_sonnet",
            ],
            Self::Codex => &["primary", "secondary"],
        }
    }
}

/// A non-empty lowercase `[a-z0-9_-]` name of at most 64 bytes.
fn slug_ok(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= 64
        && text
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}

/// Lowercases text and replaces each run of other characters with one `-`.
fn slug(text: &str) -> String {
    let mut out = String::new();
    for c in text.trim().chars().flat_map(char::to_lowercase) {
        if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-' {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let out = out.trim_matches('-');
    out[..out.len().min(64)].trim_end_matches('-').to_owned()
}

/// Claude and Codex keep their fixed windows. Any other pool is a plugin provider.
fn window_ok(pool: &str, window: &str) -> bool {
    match Pool::parse(pool) {
        Some(pool) => pool.windows().contains(&window),
        None => slug_ok(pool) && slug_ok(window),
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Row {
    /// The `CLIProxy` provider, lowercased. Consumers must accept any string.
    pub pool: String,
    pub account: String,
    pub window: String,
    pub used_pct: f64,
    pub resets_ms: Option<u64>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub version: u32,
    pub fetched_ms: u64,
    pub source: String,
    pub pools: Vec<Row>,
}

impl Snapshot {
    fn empty() -> Self {
        Self {
            version: 1,
            fetched_ms: 0,
            source: String::new(),
            pools: Vec::new(),
        }
    }

    fn validate(&self) -> Result<()> {
        if self.version != 1
            || !label(&self.source)
            || self.pools.iter().any(|row| {
                !label(&row.account)
                    || row.account.is_empty()
                    || !window_ok(&row.pool, &row.window)
                    || !row.used_pct.is_finite()
                    || !(0.0..=100.0).contains(&row.used_pct)
                    || row.error.as_ref().is_some_and(|error| !label(error))
            })
        {
            return Err(invalid_snapshot());
        }
        Ok(())
    }

    pub fn render(&self, now: u64) -> String {
        if self.fetched_ms == 0 {
            return "No quota snapshot.\n".into();
        }
        let age = now.saturating_sub(self.fetched_ms) / 1000;
        let mut out = String::new();
        for row in &self.pools {
            let reset = row
                .resets_ms
                .and_then(|ms| i64::try_from(ms).ok())
                .and_then(chrono::DateTime::from_timestamp_millis)
                .map_or_else(|| "unknown".into(), |date| date.to_rfc3339());
            let _ = writeln!(
                out,
                "{} / {} / {}: {:.1}% used, resets {reset}, age {age}s{}",
                row.pool,
                row.account,
                row.window,
                row.used_pct,
                row.error
                    .as_ref()
                    .map_or_else(String::new, |error| format!(", error: {error}"))
            );
        }
        if out.is_empty() {
            let _ = writeln!(out, "No quota accounts, age {age}s.");
        }
        out
    }
}

fn label(text: &str) -> bool {
    text.len() <= 512 && !text.chars().any(char::is_control)
}

fn invalid_snapshot() -> Error {
    Error::Invalid("invalid quota snapshot".into())
}

pub fn cached(ledger: &Ledger) -> Result<Snapshot> {
    let snapshot: Snapshot = ledger
        .read(CACHE)
        .map_err(|_| invalid_snapshot())?
        .unwrap_or_else(Snapshot::empty);
    snapshot.validate()?;
    Ok(snapshot)
}

/// Both public stdin storage and the SSH primitive use this validated writer.
pub fn store(ledger: &Ledger, input: impl std::io::Read) -> Result<Snapshot> {
    let mut bytes = Vec::new();
    input.take(MAX_SNAPSHOT + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_SNAPSHOT {
        return Err(invalid_snapshot());
    }
    let snapshot: Snapshot = serde_json::from_slice(&bytes).map_err(|_| invalid_snapshot())?;
    snapshot.validate()?;
    ledger.with_lock("quota", || {
        if cached(ledger)?.fetched_ms <= snapshot.fetched_ms {
            ledger.write(CACHE, &snapshot)?;
        }
        Ok(())
    })?;
    Ok(snapshot)
}

// Deliberately not Debug or Serialize. No generic error ever includes this value.
struct Management<'a> {
    shell: &'a dyn Shell,
    origin: &'a str,
    key: String,
}

fn words(args: &[&str]) -> Vec<String> {
    args.iter().map(ToString::to_string).collect()
}

impl<'a> Management<'a> {
    fn open(shell: &'a dyn Shell, policy: &'a Policy, macos: bool) -> Option<Self> {
        if !macos {
            return None;
        }
        let origin = policy
            .quota
            .management_url
            .as_deref()?
            .trim_end_matches('/');
        if origin.is_empty() {
            return None;
        }
        let out = shell.run("security", &words(KEY_ARGS)).ok()?;
        if !out.ok() {
            return None;
        }
        let key = out.stdout.trim().to_owned();
        if key.is_empty() || key.chars().any(char::is_control) {
            return None;
        }
        Some(Self { shell, origin, key })
    }

    fn request<T: DeserializeOwned>(&self, endpoint: &str, body: Option<&Value>) -> Collected<T> {
        // Only an HTTPS origin is configuration: paths and credentials are not accepted.
        let authority = self
            .origin
            .strip_prefix("https://")
            .filter(|value| {
                !value.is_empty()
                    && value
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b".-:[]".contains(&b))
            })
            .ok_or_else(|| "quota management_url must be an HTTPS origin".to_string())?;
        let mut config = format!(
            "silent\nshow-error\nmax-time = 25\nproto = \"=https\"\nurl = \"https://{authority}/v0/management/{endpoint}\"\nheader = {}\nwrite-out = \"\\n%{{http_code}}\"\n",
            curl_string(&format!("Authorization: Bearer {}", self.key))
        );
        if let Some(body) = body {
            let _ = writeln!(
                config,
                "header = \"Content-Type: application/json\"\ndata = {}",
                curl_string(&body.to_string())
            );
        }
        let out = self
            .shell
            .run_input_bounded(
                "curl",
                &words(CURL_ARGS),
                config.as_bytes(),
                crate::budget::CALL_LIMIT,
            )
            .map_err(|_| "quota HTTP transport failed".to_string())?;
        if !out.ok() {
            return Err("quota HTTP transport failed".into());
        }
        let (body, status) = out
            .stdout
            .rsplit_once('\n')
            .ok_or_else(|| "invalid management response".to_string())?;
        let status: u16 = status
            .trim()
            .parse()
            .map_err(|_| "invalid management status".to_string())?;
        if !(200..300).contains(&status) {
            return Err(format!("management HTTP {status}"));
        }
        serde_json::from_str(body).map_err(|_| "invalid management response".into())
    }

    fn usage(&self, pool: Pool, auth_index: &str) -> Collected<Value> {
        if auth_index.is_empty() {
            return Err("missing auth index".into());
        }
        let url = match pool {
            Pool::Claude => "https://api.anthropic.com/api/oauth/usage",
            Pool::Codex => "https://chatgpt.com/backend-api/wham/usage",
        };
        let mut header = json!({"Authorization":"Bearer $TOKEN$"});
        if pool == Pool::Claude {
            header["anthropic-beta"] = "oauth-2025-04-20".into();
        }
        let response: ApiResponse = self.request(
            "api-call",
            Some(&json!({"auth_index":auth_index,"method":"GET","url":url,"header":header})),
        )?;
        if !(200..300).contains(&response.status_code) {
            return Err(format!("upstream HTTP {}", response.status_code));
        }
        serde_json::from_str(&response.body).map_err(|_| "invalid upstream response".into())
    }

    fn rows(&self, pool: Pool, account: &str, auth_index: &str) -> Vec<Row> {
        let result = self
            .usage(pool, auth_index)
            .and_then(|body| parse_usage(pool, account, &body));
        result.unwrap_or_else(|error| error_rows(pool, account, &error))
    }

    /// `CLIProxy`'s own normalized quota route, for credentials a plugin serves.
    fn plugin_rows(&self, provider: &str, account: &str, auth_index: &str) -> Vec<Row> {
        let fail = |error: &str| plugin_error(provider, account, error);
        if auth_index.is_empty() {
            return fail("missing auth index");
        }
        let response: QuotaFetch =
            match self.request("quota/fetch", Some(&json!({"auth_index": auth_index}))) {
                Ok(response) => response,
                Err(error) => return fail(&error),
            };
        plugin_buckets(provider, account, &response).unwrap_or_else(|error| fail(&error))
    }

    fn collect(&self, now: u64, source: &str) -> Collected<Snapshot> {
        let files: AuthFiles = self.request("auth-files", None)?;
        let mut pools = Vec::new();
        for file in files.files.into_iter().filter(|file| !file.disabled) {
            let provider = file.provider.to_lowercase();
            let fixed = Pool::parse(&provider);
            if fixed.is_none() && !file.supports_quota {
                continue;
            }
            let account = [file.email, file.label, file.account, file.name]
                .into_iter()
                .flatten()
                .find(|s| !s.is_empty())
                .unwrap_or_else(|| "unnamed".into());
            // Refuse a reflected management credential even in otherwise public labels.
            let account = if label(&account) && !account.contains(&self.key) {
                account
            } else {
                "redacted".into()
            };
            pools.extend(match fixed {
                Some(pool) => self.rows(pool, &account, &file.auth_index),
                None => self.plugin_rows(&provider, &account, &file.auth_index),
            });
        }
        Ok(Snapshot {
            version: 1,
            fetched_ms: now,
            source: source.into(),
            pools,
        })
    }
}

fn curl_string(text: &str) -> String {
    format!(
        "\"{}\"",
        text.replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
            .replace('\r', "\\r")
            .replace('\t', "\\t")
    )
}

#[derive(Deserialize)]
struct ApiResponse {
    status_code: u16,
    body: String,
}

#[derive(Deserialize)]
struct AuthFiles {
    files: Vec<AuthFile>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct AuthFile {
    provider: String,
    auth_index: String,
    disabled: bool,
    supports_quota: bool,
    email: Option<String>,
    label: Option<String>,
    account: Option<String>,
    name: Option<String>,
}

/// `CLIProxy` `QuotaFetchResponse`: only the fields dogsled reads.
#[derive(Default, Deserialize)]
#[serde(default)]
struct QuotaFetch {
    groups: Vec<QuotaGroup>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct QuotaGroup {
    #[serde(alias = "display_name", rename = "displayName")]
    display_name: String,
    buckets: Vec<QuotaBucket>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct QuotaBucket {
    window: String,
    #[serde(alias = "remaining_fraction", rename = "remainingFraction")]
    remaining_fraction: Option<f64>,
    #[serde(alias = "reset_time", rename = "resetTime")]
    reset_time: Option<String>,
}

fn plugin_error(provider: &str, account: &str, error: &str) -> Vec<Row> {
    vec![Row {
        pool: provider.into(),
        account: account.into(),
        window: PLUGIN_ERROR_WINDOW.into(),
        used_pct: 0.0,
        resets_ms: None,
        error: Some(error.into()),
    }]
}

fn plugin_buckets(provider: &str, account: &str, response: &QuotaFetch) -> Collected<Vec<Row>> {
    let grouped = response.groups.len() > 1;
    let mut rows = Vec::new();
    for group in &response.groups {
        for bucket in &group.buckets {
            let window = slug(&bucket.window);
            let window = if grouped {
                let group = slug(&group.display_name);
                if group.is_empty() {
                    return Err("invalid quota group".into());
                }
                format!("{group}-{window}")
            } else {
                window
            };
            let remaining = bucket
                .remaining_fraction
                .filter(|n| n.is_finite())
                .ok_or_else(|| "invalid quota usage".to_string())?;
            let resets_ms = match bucket.reset_time.as_deref().filter(|t| !t.is_empty()) {
                None => None,
                Some(text) => Some(
                    chrono::DateTime::parse_from_rfc3339(text)
                        .ok()
                        .and_then(|date| u64::try_from(date.timestamp_millis()).ok())
                        .ok_or_else(|| "invalid quota reset".to_string())?,
                ),
            };
            rows.push(Row {
                pool: provider.into(),
                account: account.into(),
                window,
                used_pct: ((1.0 - remaining) * 100.0).clamp(0.0, 100.0),
                resets_ms,
                error: None,
            });
        }
    }
    if rows.is_empty() || rows.iter().any(|row| !window_ok(&row.pool, &row.window)) {
        return Err("no quota windows".into());
    }
    Ok(rows)
}

fn error_rows(pool: Pool, account: &str, error: &str) -> Vec<Row> {
    pool.windows()
        .iter()
        .map(|window| Row {
            pool: pool.name().into(),
            account: account.into(),
            window: (*window).into(),
            used_pct: 0.0,
            resets_ms: None,
            error: Some(error.into()),
        })
        .collect()
}

fn parse_usage(pool: Pool, account: &str, body: &Value) -> Collected<Vec<Row>> {
    let mut rows = Vec::new();
    for window in pool.windows() {
        let (value, used_key, reset_key) = match pool {
            Pool::Claude => (&body[*window], "utilization", "resets_at"),
            Pool::Codex => (
                &body["rate_limit"][format!("{window}_window")],
                "used_percent",
                "reset_at",
            ),
        };
        if value.is_null() {
            continue;
        }
        let used = value[used_key]
            .as_f64()
            .filter(|n| n.is_finite())
            .ok_or_else(|| "invalid quota usage".to_string())?;
        let reset = &value[reset_key];
        let resets_ms = if reset.is_null() {
            None
        } else {
            Some(
                match pool {
                    Pool::Claude => reset
                        .as_str()
                        .and_then(|text| chrono::DateTime::parse_from_rfc3339(text).ok())
                        .and_then(|date| u64::try_from(date.timestamp_millis()).ok()),
                    Pool::Codex => reset.as_u64().and_then(|seconds| seconds.checked_mul(1000)),
                }
                .ok_or_else(|| "invalid quota reset".to_string())?,
            )
        };
        rows.push(Row {
            pool: pool.name().into(),
            account: account.into(),
            window: (*window).into(),
            used_pct: used.clamp(0.0, 100.0),
            resets_ms,
            error: None,
        });
    }
    if rows.is_empty() {
        return Err("no quota windows".into());
    }
    Ok(rows)
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Refresh {
    attempted_ms: u64,
    attempts: u64,
    retry: Backoff,
    pending_push: bool,
    push_retry: Backoff,
}

pub fn command(
    shell: &dyn Shell,
    ledger: &Ledger,
    policy: &Policy,
    home: &str,
    args: &crate::cli::QuotaArgs,
) -> Result<Snapshot> {
    if args.store {
        store(ledger, std::io::stdin().lock())
    } else {
        read(shell, ledger, policy, home, args.cached, args.refresh)
    }
}

pub fn read(
    shell: &dyn Shell,
    ledger: &Ledger,
    policy: &Policy,
    home: &str,
    only_cached: bool,
    force: bool,
) -> Result<Snapshot> {
    Collector {
        shell,
        ledger,
        policy,
        home,
        macos: cfg!(target_os = "macos"),
    }
    .read(only_cached, force, crate::ledger::now_ms())
}

struct Collector<'a> {
    shell: &'a dyn Shell,
    ledger: &'a Ledger,
    policy: &'a Policy,
    home: &'a str,
    macos: bool,
}

impl Collector<'_> {
    fn read(&self, only_cached: bool, force: bool, now: u64) -> Result<Snapshot> {
        let previous = cached(self.ledger)?;
        if only_cached || !self.macos || self.policy.quota.management_url.is_none() {
            return Ok(previous);
        }
        let observed: Refresh = self.ledger.read("quota-refresh.json")?.unwrap_or_default();
        self.ledger.with_lock("quota", || {
            let mut snapshot = cached(self.ledger)?;
            let mut refresh: Refresh = self.ledger.read("quota-refresh.json")?.unwrap_or_default();
            let fresh =
                snapshot.fetched_ms != 0 && now.saturating_sub(snapshot.fetched_ms) < REFRESH_MS;
            let collect = (force || (!fresh && refresh.retry.ready(now)))
                && refresh.attempts == observed.attempts;
            let deliver = refresh.pending_push && (force || refresh.push_retry.ready(now));
            if !collect && !deliver {
                return Ok(snapshot);
            }
            let Some(client) = Management::open(self.shell, self.policy, self.macos) else {
                return Ok(snapshot);
            };
            if collect {
                refresh.attempted_ms = now;
                refresh.attempts = refresh.attempts.saturating_add(1);
                match client.collect(now, &crate::inbox::hostname()) {
                    Ok(collected) => {
                        collected.validate()?;
                        self.ledger.write(CACHE, &collected)?;
                        refresh.retry.reset();
                        refresh.pending_push = true;
                        refresh.push_retry.reset();
                        snapshot = collected;
                    }
                    Err(error) => {
                        refresh.retry.fail(now, "quota");
                        self.ledger.write("quota-refresh.json", &refresh)?;
                        return Err(Error::Invalid(error));
                    }
                }
            }
            if refresh.pending_push {
                // Persist before transport so a crash leaves delivery pending.
                self.ledger.write("quota-refresh.json", &refresh)?;
                match push(self.shell, self.ledger, self.policy, self.home, &snapshot) {
                    Ok(()) => {
                        refresh.pending_push = false;
                        refresh.push_retry.reset();
                    }
                    Err(error) => {
                        refresh.push_retry.fail(now, "quota-push");
                        self.ledger.write("quota-refresh.json", &refresh)?;
                        return Err(error);
                    }
                }
            }
            self.ledger.write("quota-refresh.json", &refresh)?;
            Ok(snapshot)
        })
    }
}

fn push(
    shell: &dyn Shell,
    ledger: &Ledger,
    policy: &Policy,
    home: &str,
    snapshot: &Snapshot,
) -> Result<()> {
    let fleet = Fleet::discover(shell, ledger, policy, home.into())
        .map_err(|_| Error::Invalid("quota host discovery failed".into()))?;
    let input = serde_json::to_vec(snapshot)?;
    let mut failed = false;
    for host in fleet.hosts.iter().filter(|host| !host.is_local()) {
        let args = fleet.remote_argv(host, &["quota-store".into()]);
        failed |= !shell
            .run_input_bounded("ssh", &args, &input, crate::budget::CALL_LIMIT)
            .is_ok_and(|output| output.ok());
    }
    if failed {
        return Err(Error::Invalid("quota snapshot push failed".into()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::Output;
    use std::collections::VecDeque;
    use std::sync::Mutex;

    const SECRET: &str = "quota-secret-sentinel";
    const RAW_KEY: &str = "plugin-secret-sentinel";

    #[derive(Debug, Default)]
    struct HttpShell {
        shell: crate::shell::Fake,
        replies: Mutex<VecDeque<Output>>,
        requests: Mutex<Vec<String>>,
        stored: Mutex<Vec<Snapshot>>,
        fail_push: Mutex<bool>,
    }

    impl HttpShell {
        fn new() -> Self {
            let shell = Self::default();
            shell
                .shell
                .on(&format!("security {}", KEY_ARGS.join(" ")), SECRET);
            shell.shell.on("herdr machine list --json", r#"[{"id":"peer","label":"peer","target":"peer.example"},{"id":"off","label":"off","target":"off.example","enabled":false}]"#);
            shell
        }

        fn reply(&self, value: &Value, status: u16) {
            self.replies.lock().unwrap().push_back(Output {
                status: 0,
                stdout: format!("{value}\n{status}"),
                stderr: SECRET.into(),
            });
        }

        fn usage(&self, value: &Value, status: u16) {
            self.reply(
                &json!({"status_code":status,"header":{"secret":SECRET},"body":value.to_string()}),
                200,
            );
        }

        fn minimal(&self) {
            self.reply(&json!({"files":[]}), 200);
        }
    }

    impl Shell for HttpShell {
        fn run(&self, program: &str, args: &[String]) -> Result<Output> {
            self.shell.run(program, args)
        }

        fn run_input_bounded(
            &self,
            program: &str,
            args: &[String],
            input: &[u8],
            _: std::time::Duration,
        ) -> Result<Output> {
            assert!(!args.join(" ").contains(SECRET));
            assert!(!args.join(" ").contains(RAW_KEY));
            if program == "ssh" {
                assert!(args.join(" ").contains("host quota-store --json"));
                assert!(!args.join(" ").contains("off.example"));
                let snapshot: Snapshot = serde_json::from_slice(input).unwrap();
                self.stored.lock().unwrap().push(snapshot);
                return Ok(Output {
                    status: i32::from(*self.fail_push.lock().unwrap()),
                    stdout: "{}".into(),
                    stderr: SECRET.into(),
                });
            }
            assert_eq!(program, "curl");
            assert_eq!(args, words(CURL_ARGS));
            let config = String::from_utf8(input.to_vec()).unwrap();
            assert!(config.contains(&format!("header = \"Authorization: Bearer {SECRET}\"")));
            self.requests.lock().unwrap().push(config);
            Ok(self.replies.lock().unwrap().pop_front().unwrap())
        }
    }

    struct Fixture {
        shell: HttpShell,
        ledger: Ledger,
        policy: Policy,
    }

    impl Fixture {
        fn new() -> Self {
            let mut policy = Policy::default();
            policy.quota.management_url = Some("https://proxy.example".into());
            Self {
                shell: HttpShell::new(),
                ledger: Ledger::new(std::env::temp_dir().join(crate::ledger::new_id())),
                policy,
            }
        }

        fn collector(&self) -> Collector<'_> {
            Collector {
                shell: &self.shell,
                ledger: &self.ledger,
                policy: &self.policy,
                home: "/home/test",
                macos: true,
            }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.ledger.dir);
        }
    }

    #[test]
    fn quota_collects_all_providers_and_never_persists_secrets() {
        let f = Fixture::new();
        f.shell.reply(
            &json!({"files":[
                {"provider":"claude","email":"claude@example.com","auth_index":"c","extra":SECRET},
                {"provider":"codex","email":"codex@example.com","auth_index":"o"},
                {"provider":"claude","email":"disabled","auth_index":"d","disabled":true},
                {"provider":"other","email":"skip"}
            ]}),
            200,
        );
        f.shell.usage(
            &json!({
                "five_hour":{"utilization":25.5,"resets_at":"2026-01-01T01:00:00+01:00"},
                "seven_day":{"utilization":101,"resets_at":null},
                "seven_day_opus":{"utilization":12,"resets_at":"2026-01-02T00:00:00.125Z"},
                "seven_day_sonnet":{"utilization":-1,"resets_at":null},
                "unknown":{"utilization":99,"secret":SECRET}
            }),
            200,
        );
        f.shell.usage(
            &json!({"rate_limit":{
                "primary_window":{"used_percent":75,"reset_at":1_800_000_000_u64},
                "secondary_window":{"used_percent":20,"reset_at":null}
            }}),
            200,
        );
        let snapshot = f.collector().read(false, false, 1_000_000).unwrap();
        assert_eq!(snapshot.pools.len(), 6);
        assert_eq!(snapshot.pools[0].resets_ms, Some(1_767_225_600_000));
        assert_eq!(snapshot.pools[0].used_pct, 25.5);
        assert_eq!(snapshot.pools[1].used_pct, 100.0);
        assert_eq!(snapshot.pools[2].resets_ms, Some(1_767_312_000_125));
        assert_eq!(snapshot.pools[3].used_pct, 0.0);
        assert_eq!(snapshot.pools[4].resets_ms, Some(1_800_000_000_000));
        assert_eq!(snapshot.pools[5].used_pct, 20.0);
        assert_eq!(
            f.shell.stored.lock().unwrap().as_slice(),
            std::slice::from_ref(&snapshot)
        );
        assert_eq!(cached(&f.ledger).unwrap(), snapshot);
        let printed = serde_json::to_string(&snapshot).unwrap() + &snapshot.render(1_001_000);
        assert!(!printed.contains(SECRET));
        assert!(!printed.contains(RAW_KEY));
        for entry in std::fs::read_dir(&f.ledger.dir).unwrap() {
            let text = std::fs::read_to_string(entry.unwrap().path()).unwrap();
            assert!(!text.contains(SECRET));
            assert!(!text.contains(RAW_KEY));
        }
        let requests = f.shell.requests.lock().unwrap();
        assert!(requests[1].contains("anthropic-beta"));
        assert!(requests[1].contains("oauth-2025-04-20"));
        assert!(requests[1].contains("$TOKEN$"));
        assert!(requests[2].contains("wham/usage"));
        assert_eq!(requests.len(), 3);
        assert!(requests.iter().all(|r| !r.contains("claude-api-key")));
    }

    #[test]
    fn quota_per_account_errors_are_redacted_and_do_not_fail_collection() {
        let f = Fixture::new();
        f.shell.reply(
            &json!({"files":[
                {"provider":"claude","label":SECRET,"auth_index":"c"},
                {"provider":"codex","email":"codex","auth_index":"o"}
            ]}),
            200,
        );
        f.shell.usage(&json!({"error":SECRET}), 401);
        f.shell.reply(&json!({"error":SECRET}), 503);
        let snapshot = f.collector().read(false, false, 1000).unwrap();
        assert_eq!(snapshot.pools.len(), 6);
        assert!(snapshot.pools.iter().all(|row| row.error.is_some()));
        assert_eq!(
            snapshot.pools[0].error.as_deref(),
            Some("upstream HTTP 401")
        );
        assert_eq!(
            snapshot.pools[4].error.as_deref(),
            Some("management HTTP 503")
        );
        let printed = serde_json::to_string(&snapshot).unwrap() + &snapshot.render(1000);
        assert!(!printed.contains(SECRET));
        assert!(!printed.contains(RAW_KEY));
    }

    fn plugin_files() -> Value {
        json!({"files":[
            {"provider":"claude","email":"c@example.com","auth_index":"c","supports_quota":true},
            {"provider":"MiniMax","email":"mm@example.com","auth_index":"p","supports_quota":true,"quota_provider":"minimax"},
            {"provider":"minimax","email":"off","auth_index":"x","supports_quota":true,"disabled":true},
            {"provider":"other","email":"none","auth_index":"n"}
        ]})
    }

    #[test]
    fn quota_plugin_credential_maps_groups_and_buckets_to_rows() {
        let f = Fixture::new();
        f.shell.reply(&plugin_files(), 200);
        f.shell.usage(
            &json!({"five_hour":{"utilization":10,"resets_at":null}}),
            200,
        );
        f.shell.reply(
            &json!({"groups":[
                {"displayName":"Text Models","buckets":[
                    {"window":"interval","remainingFraction":0.25,"resetTime":"2026-01-01T00:00:00Z"},
                    {"window":"weekly","remainingFraction":-0.5}]},
                {"display_name":"Image","buckets":[
                    {"window":"daily","remaining_fraction":1.5,"reset_time":"2026-01-02T00:00:00Z"}]}
            ]}),
            200,
        );
        let snapshot = f.collector().read(false, false, 1000).unwrap();
        let plugin: Vec<_> = snapshot
            .pools
            .iter()
            .filter(|r| r.pool == "minimax")
            .collect();
        assert_eq!(plugin.len(), 3);
        assert_eq!(plugin[0].account, "mm@example.com");
        assert_eq!(plugin[0].window, "text-models-interval");
        assert_eq!(plugin[0].used_pct, 75.0);
        assert_eq!(plugin[0].resets_ms, Some(1_767_225_600_000));
        assert_eq!(plugin[1].window, "text-models-weekly");
        assert_eq!(plugin[1].used_pct, 100.0);
        assert_eq!(plugin[1].resets_ms, None);
        assert_eq!(plugin[2].window, "image-daily");
        assert_eq!(plugin[2].used_pct, 0.0);
        assert!(plugin.iter().all(|r| r.error.is_none()));
        assert_eq!(cached(&f.ledger).unwrap(), snapshot);
        let requests = f.shell.requests.lock().unwrap();
        // auth-files, claude api-call, one plugin fetch: claude keeps api-call, and
        // disabled and unsupported credentials are skipped.
        assert_eq!(requests.len(), 3);
        assert!(requests[1].contains("/v0/management/api-call"));
        assert!(requests[2].contains("/v0/management/quota/fetch"));
        assert!(requests[2].contains(r#"{\"auth_index\":\"p\"}"#));
        assert!(requests.iter().all(|r| !r.contains("claude-api-key")));
        assert!(!requests.iter().any(|r| r.contains(r#"\"x\""#)));
    }

    #[test]
    fn quota_plugin_single_group_uses_bare_window() {
        let f = Fixture::new();
        f.shell.reply(
            &json!({"files":[{"provider":"minimax","email":"m","auth_index":"p","supports_quota":true}]}),
            200,
        );
        f.shell.reply(
            &json!({"groups":[{"displayName":"All","buckets":[{"window":"5h","remainingFraction":0.5}]}]}),
            200,
        );
        let snapshot = f.collector().read(false, false, 1000).unwrap();
        assert_eq!(snapshot.pools.len(), 1);
        assert_eq!(snapshot.pools[0].window, "5h");
        assert_eq!(snapshot.pools[0].used_pct, 50.0);
    }

    #[test]
    fn quota_plugin_fetch_failures_become_redacted_error_rows() {
        for (status, body) in [
            (502, json!({"error":SECRET})),
            (501, json!({"error":RAW_KEY})),
            (200, json!({"groups":[]})),
            (
                200,
                json!({"groups":[{"displayName":"a","buckets":[{"window":"w"}]}]}),
            ),
        ] {
            let f = Fixture::new();
            f.shell.reply(
                &json!({"files":[
                    {"provider":"minimax","label":SECRET,"auth_index":"p","supports_quota":true},
                    {"provider":"zed","email":"z","auth_index":"q","supports_quota":true}
                ]}),
                200,
            );
            f.shell.reply(&body, status);
            f.shell.reply(
                &json!({"groups":[{"displayName":"a","buckets":[{"window":"w","remainingFraction":0.9}]}]}),
                200,
            );
            let snapshot = f.collector().read(false, false, 1000).unwrap();
            assert_eq!(snapshot.pools.len(), 2);
            assert_eq!(snapshot.pools[0].pool, "minimax");
            assert_eq!(snapshot.pools[0].account, "redacted");
            assert!(snapshot.pools[0].error.is_some());
            assert_eq!(snapshot.pools[0].used_pct, 0.0);
            // One failing account does not hide the next one.
            assert_eq!(snapshot.pools[1].pool, "zed");
            assert!(snapshot.pools[1].error.is_none());
            let printed = serde_json::to_string(&snapshot).unwrap() + &snapshot.render(1000);
            assert!(!printed.contains(SECRET));
            assert!(!printed.contains(RAW_KEY));
            for entry in std::fs::read_dir(&f.ledger.dir).unwrap() {
                let text = std::fs::read_to_string(entry.unwrap().path()).unwrap();
                assert!(!text.contains(SECRET));
                assert!(!text.contains(RAW_KEY));
            }
        }
    }

    #[test]
    fn quota_plugin_rows_validate_and_slugs_are_safe() {
        assert_eq!(slug(" Text  Models!/"), "text-models");
        assert_eq!(slug("A_b-C9"), "a_b-c9");
        assert_eq!(slug("!!!"), "");
        assert!(window_ok("minimax", "text-models-interval"));
        assert!(!window_ok("minimax", "Bad Window"));
        assert!(!window_ok("Bad Pool", "w"));
        assert!(!window_ok("claude", "interval"));
        assert!(window_ok("codex", "primary"));
        let f = Fixture::new();
        let snapshot = Snapshot {
            fetched_ms: 1,
            source: "mac".into(),
            pools: vec![Row {
                pool: "minimax".into(),
                account: "a".into(),
                window: "interval".into(),
                used_pct: 1.0,
                resets_ms: None,
                error: None,
            }],
            version: 1,
        };
        assert_eq!(
            store(&f.ledger, serde_json::to_vec(&snapshot).unwrap().as_slice()).unwrap(),
            snapshot
        );
    }

    #[test]
    fn quota_cached_and_inactive_collectors_never_use_network() {
        let mut f = Fixture::new();
        let snapshot = Snapshot {
            fetched_ms: 1,
            source: "laptop".into(),
            ..Snapshot::empty()
        };
        f.ledger.write(CACHE, &snapshot).unwrap();
        assert_eq!(
            f.collector().read(true, false, 2_000_000).unwrap(),
            snapshot
        );
        assert!(f.shell.shell.calls.lock().unwrap().is_empty());
        f.policy.quota.management_url = None;
        assert_eq!(
            f.collector().read(false, true, 2_000_000).unwrap(),
            snapshot
        );
        assert!(f.shell.shell.calls.lock().unwrap().is_empty());
        f.policy.quota.management_url = Some("https://proxy.example".into());
        let mut linux = f.collector();
        linux.macos = false;
        assert_eq!(linux.read(false, true, 2_000_000).unwrap(), snapshot);
        assert!(f.shell.shell.calls.lock().unwrap().is_empty());
        f.shell.shell.on_output(
            &format!("security {}", KEY_ARGS.join(" ")),
            Output {
                status: 44,
                stdout: SECRET.into(),
                stderr: SECRET.into(),
            },
        );
        assert_eq!(
            f.collector().read(false, true, 2_000_000).unwrap(),
            snapshot
        );
        assert!(f.shell.requests.lock().unwrap().is_empty());
        assert_eq!(f.shell.shell.calls.lock().unwrap().len(), 1);
    }

    #[test]
    fn quota_store_roundtrips_and_rejects_invalid_snapshots_without_echo() {
        let f = Fixture::new();
        let snapshot = Snapshot {
            fetched_ms: 1000,
            source: "mac".into(),
            pools: vec![Row {
                pool: "claude".into(),
                account: "me".into(),
                window: "five_hour".into(),
                used_pct: 100.0,
                resets_ms: Some(2000),
                error: None,
            }],
            version: 1,
        };
        let bytes = serde_json::to_vec(&snapshot).unwrap();
        assert_eq!(store(&f.ledger, bytes.as_slice()).unwrap(), snapshot);
        assert_eq!(cached(&f.ledger).unwrap(), snapshot);
        let older = serde_json::to_vec(&Snapshot::empty()).unwrap();
        store(&f.ledger, older.as_slice()).unwrap();
        assert_eq!(cached(&f.ledger).unwrap(), snapshot);
        for bad in [
            json!({"secret":SECRET}),
            json!({"version":2,"fetched_ms":0,"source":SECRET,"pools":[]}),
        ] {
            let bytes = bad.to_string();
            let error = store(&f.ledger, bytes.as_bytes()).unwrap_err().to_string();
            assert!(!error.contains(SECRET));
            assert_eq!(cached(&f.ledger).unwrap(), snapshot);
        }
        let mut invalid = snapshot;
        invalid.pools[0].used_pct = 101.0;
        assert!(store(&f.ledger, serde_json::to_vec(&invalid).unwrap().as_slice()).is_err());
    }

    #[test]
    fn quota_refresh_age_force_backoff_and_push_retry() {
        let f = Fixture::new();
        f.shell.minimal();
        *f.shell.fail_push.lock().unwrap() = true;
        assert!(f.collector().read(false, false, 1000).is_err());
        assert_eq!(cached(&f.ledger).unwrap().fetched_ms, 1000);
        *f.shell.fail_push.lock().unwrap() = false;
        f.collector().read(false, false, 1001).unwrap();
        assert_eq!(f.shell.stored.lock().unwrap().len(), 1);
        f.collector().read(false, false, 20_000).unwrap();
        assert_eq!(f.shell.stored.lock().unwrap().len(), 2);
        assert_eq!(f.shell.requests.lock().unwrap().len(), 1);
        f.shell.minimal();
        f.collector().read(false, true, 20_001).unwrap();
        assert_eq!(f.shell.requests.lock().unwrap().len(), 2);
        f.shell.reply(&json!({"error":SECRET}), 500);
        let error = f
            .collector()
            .read(false, false, 400_000)
            .unwrap_err()
            .to_string();
        assert!(!error.contains(SECRET));
        assert_eq!(cached(&f.ledger).unwrap().fetched_ms, 20_001);
        f.collector().read(false, false, 400_001).unwrap();
        assert_eq!(f.shell.requests.lock().unwrap().len(), 3);
        f.shell.minimal();
        f.collector().read(false, false, 420_000).unwrap();
        assert_eq!(f.shell.requests.lock().unwrap().len(), 4);
    }

    #[test]
    fn quota_concurrent_refreshes_collect_once() {
        let f = Fixture::new();
        f.shell.minimal();
        std::thread::scope(|scope| {
            for _ in 0..8 {
                let fixture = &f;
                scope.spawn(move || fixture.collector().read(false, false, 1000).unwrap());
            }
        });
        assert_eq!(f.shell.requests.lock().unwrap().len(), 1);
        assert_eq!(f.shell.stored.lock().unwrap().len(), 1);
    }

    #[test]
    fn quota_bad_bodies_and_transport_errors_never_echo_secrets() {
        let f = Fixture::new();
        f.shell.replies.lock().unwrap().push_back(Output {
            status: 7,
            stdout: SECRET.into(),
            stderr: RAW_KEY.into(),
        });
        let error = f
            .collector()
            .read(false, true, 1000)
            .unwrap_err()
            .to_string();
        assert!(!error.contains(SECRET));
        assert!(!error.contains(RAW_KEY));
        f.shell.replies.lock().unwrap().push_back(Output {
            status: 0,
            stdout: format!("{SECRET}\n200"),
            stderr: RAW_KEY.into(),
        });
        let error = f
            .collector()
            .read(false, true, 1001)
            .unwrap_err()
            .to_string();
        assert!(!error.contains(SECRET));
        assert!(!error.contains(RAW_KEY));
    }

    #[test]
    fn quota_shell_stdin_is_bounded_and_absent_from_diagnostics() {
        let shell = crate::shell::System::default();
        let out = shell
            .run_input_bounded(
                "cat",
                &[],
                SECRET.as_bytes(),
                std::time::Duration::from_secs(2),
            )
            .unwrap();
        assert_eq!(out.stdout, SECRET);
        let error = shell
            .run_input_bounded(
                "sleep",
                &["5".into()],
                &vec![b'x'; 1024 * 1024],
                std::time::Duration::from_millis(50),
            )
            .unwrap_err();
        assert!(matches!(error, Error::Deadline(_)));
        assert!(!error.to_string().contains(SECRET));
        let out = shell.run("cat", &[]).unwrap();
        assert_eq!(out.stdout, "");
    }

    #[test]
    fn quota_config_and_cli_contract() {
        use clap::Parser as _;
        let policy =
            crate::config::parse("[quota]\nmanagement_url = 'https://proxy.example'\n").unwrap();
        assert_eq!(
            policy.quota.management_url.as_deref(),
            Some("https://proxy.example")
        );
        assert!(crate::config::parse("[quota]\nmanagement_key = 'forbidden'\n").is_err());
        for args in [
            vec!["dogsled", "quota", "--json", "--cached"],
            vec!["dogsled", "quota", "--refresh"],
            vec!["dogsled", "quota", "--store"],
        ] {
            assert!(crate::cli::Cli::try_parse_from(args).is_ok());
        }
        assert!(
            crate::cli::Cli::try_parse_from(["dogsled", "quota", "--cached", "--refresh"]).is_err()
        );
        assert!(
            crate::cli::Cli::try_parse_from(["dogsled", "quota", "--cached", "--store"]).is_err()
        );
    }
}
