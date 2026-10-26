//! Host-owned job execution, output capture, and bounded process-group lifetime.

use crate::error::{Error, Result};
use crate::ledger::{Ledger, now_ms};
use crate::shell::{Shell, kill_group};
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Job {
    pub id: String,
    pub cmd: Vec<String>,
    pub cwd: String,
    pub host: String,
    pub owner: String,
    pub pane: String,
    pub state: String,
    pub exit: Option<i32>,
    pub signal: Option<i32>,
    pub reason: Option<String>,
    pub started_ms: Option<u64>,
    pub ended_ms: Option<u64>,
    pub timeout_ms: Option<u64>,
    pub idle_timeout_ms: Option<u64>,
}

pub fn duration(text: &str) -> std::result::Result<u64, String> {
    let (number, factor) = if let Some(n) = text.strip_suffix("ms") {
        (n, 1)
    } else if let Some(n) = text.strip_suffix('s') {
        (n, 1_000)
    } else if let Some(n) = text.strip_suffix('m') {
        (n, 60_000)
    } else if let Some(n) = text.strip_suffix('h') {
        (n, 3_600_000)
    } else {
        return Err("duration needs ms, s, m, or h".into());
    };
    number
        .parse::<u64>()
        .ok()
        .and_then(|n| n.checked_mul(factor))
        .ok_or_else(|| "invalid duration".into())
}

pub fn store(ledger: &Ledger) -> Ledger {
    ledger.child("jobs")
}

pub fn validate_id(id: &str) -> Result<()> {
    if id.is_empty() || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
        return Err(Error::Invalid("invalid job id".into()));
    }
    Ok(())
}

pub fn get(ledger: &Ledger, id: &str) -> Result<Job> {
    validate_id(id)?;
    store(ledger)
        .read(&format!("{id}.json"))?
        .ok_or_else(|| Error::Invalid(format!("unknown job {id}")))
}

pub fn save(ledger: &Ledger, job: &Job) -> Result<()> {
    validate_id(&job.id)?;
    store(ledger).write(&format!("{}.json", job.id), job)
}

pub fn list(ledger: &Ledger) -> Result<Vec<Job>> {
    let store = store(ledger);
    let entries = match fs::read_dir(&store.dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    let mut jobs = Vec::new();
    for entry in entries {
        let entry = entry?;
        if entry.path().extension().is_some_and(|e| e == "json") {
            let job: Job = serde_json::from_slice(&fs::read(entry.path())?)?;
            jobs.push(job);
        }
    }
    jobs.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(jobs)
}

pub fn stop(ledger: &Ledger, id: &str) -> Result<()> {
    validate_id(id)?;
    store(ledger).with_lock(id, || {
        let job = get(ledger, id)?;
        if job.ended_ms.is_none() {
            store(ledger).write(&format!("{id}.stop"), &true)?;
        }
        Ok(())
    })
}

#[derive(Debug, Serialize, Deserialize)]
pub struct LogChunk {
    pub bytes: Vec<u8>,
    pub next: u64,
    pub done: bool,
}

pub fn logs(ledger: &Ledger, id: &str, offset: u64) -> Result<LogChunk> {
    use std::io::{Seek, SeekFrom};
    let job = get(ledger, id)?;
    let mut bytes = Vec::new();
    match fs::File::open(store(ledger).dir.join(format!("{id}.log"))) {
        Ok(mut file) => {
            file.seek(SeekFrom::Start(offset))?;
            file.take(64 * 1024).read_to_end(&mut bytes)?;
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && job.ended_ms.is_none() => (),
        Err(e) => return Err(e.into()),
    }
    Ok(LogChunk {
        next: offset + bytes.len() as u64,
        bytes,
        done: job.ended_ms.is_some(),
    })
}

fn report(shell: &dyn Shell, job: &Job, state: &str, token: Option<&str>) -> Result<()> {
    if job.pane.is_empty() {
        return Ok(());
    }
    shell.run_ok(
        "herdr",
        &[
            "pane".into(),
            "report-agent".into(),
            job.pane.clone(),
            "--source".into(),
            "custom:dogsled".into(),
            "--agent".into(),
            "dogsled-job".into(),
            "--state".into(),
            state.into(),
        ],
    )?;
    let mut args = vec![
        "pane".into(),
        "report-metadata".into(),
        job.pane.clone(),
        "--source".into(),
        "custom:dogsled".into(),
        "--display-agent".into(),
        job.cmd.join(" ").chars().take(60).collect(),
    ];
    if let Some(token) = token {
        args.extend(["--token".into(), format!("dogsled={token}")]);
    }
    shell.run_ok("herdr", &args)?;
    Ok(())
}

fn reader(mut input: impl Read + Send + 'static, send: mpsc::Sender<std::io::Result<Vec<u8>>>) {
    std::thread::spawn(move || {
        let mut buffer = [0; 8192];
        loop {
            match input.read(&mut buffer) {
                Ok(0) => break,
                Ok(n) => {
                    if send.send(Ok(buffer[..n].to_vec())).is_err() {
                        break;
                    }
                }
                Err(e) => {
                    let _ = send.send(Err(e));
                    break;
                }
            }
        }
    });
}

fn pane_write(bytes: &[u8]) -> bool {
    std::io::stdout()
        .write_all(bytes)
        .and_then(|()| std::io::stdout().flush())
        .is_ok()
}

fn capture(ledger: &Ledger, job: &Job) -> Result<(std::process::ExitStatus, Option<String>)> {
    let mut signals = signal_hook::iterator::Signals::new([
        signal_hook::consts::SIGINT,
        signal_hook::consts::SIGTERM,
        signal_hook::consts::SIGHUP,
    ])?;
    let mut log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(store(ledger).dir.join(format!("{}.log", job.id)))?;
    let mut child = Command::new(&job.cmd[0])
        .args(&job.cmd[1..])
        .current_dir(&job.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()?;
    let result = (|| {
        let (send, recv) = mpsc::channel();
        reader(
            child
                .stdout
                .take()
                .ok_or_else(|| Error::Invalid("missing stdout".into()))?,
            send.clone(),
        );
        reader(
            child
                .stderr
                .take()
                .ok_or_else(|| Error::Invalid("missing stderr".into()))?,
            send,
        );
        let start = Instant::now();
        let mut last = start;
        let mut reason = None;
        let mut status = None;
        let mut disconnected = false;
        let mut pane_output = true;
        loop {
            if status.is_none() {
                status = child.try_wait()?;
                if status.is_some() {
                    kill_group(child.id())?;
                }
            }
            if status.is_some() && disconnected {
                break;
            }
            if status.is_none() && reason.is_none() {
                reason = if let Some(signal) = signals.pending().next() {
                    Some(format!("signal {signal}"))
                } else if store(ledger).dir.join(format!("{}.stop", job.id)).exists() {
                    Some("stopped".into())
                } else if job
                    .timeout_ms
                    .is_some_and(|ms| start.elapsed() >= Duration::from_millis(ms))
                {
                    Some("timeout".into())
                } else if job
                    .idle_timeout_ms
                    .is_some_and(|ms| last.elapsed() >= Duration::from_millis(ms))
                {
                    Some("idle-timeout".into())
                } else {
                    None
                };
                if reason.is_some() {
                    kill_group(child.id())?;
                }
            }
            match recv.recv_timeout(Duration::from_millis(10)) {
                Ok(bytes) => {
                    let bytes = bytes?;
                    log.write_all(&bytes)?;
                    if pane_output {
                        pane_output = pane_write(&bytes);
                    }
                    last = Instant::now();
                }
                Err(mpsc::RecvTimeoutError::Timeout) => (),
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    disconnected = true;
                    if status.is_none() {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                }
            }
        }
        log.sync_all()?;
        Ok((
            status.ok_or_else(|| Error::Invalid("missing child status".into()))?,
            reason,
        ))
    })();
    if result.is_err() {
        kill_group(child.id())?;
        child.wait()?;
    }
    result
}

/// A job record that has not started: the intent published before its pane is known. It is
/// not executable, and no wrapper may run a job from it.
pub const CLAIMING: &str = "claiming";
/// An executable claim: the pane is known, and the command may run.
pub const PENDING: &str = "pending";
/// The wrapper owns the job and runs it.
pub const WORKING: &str = "working";
/// A claim whose dispatch outcome could not be proved.
pub const UNCERTAIN: &str = "uncertain";

/// What a job's durable record says about who may run it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ownership {
    /// No record: nobody claimed this job.
    Unclaimed,
    /// An executable claim. The wrapper may run it.
    Pending(Job),
    /// The wrapper owns it and runs it.
    Started(Job),
    /// It ended.
    Terminal(Job),
    /// It holds an ownership that no execution may take over: an allocation that stopped
    /// before its pane was known, or a dispatch whose outcome was never proved.
    Uncertain(Job),
}

/// The durable record of a job. A missing record is `None`; an unreadable one is an error,
/// so a caller never mistakes a corrupt record for an absent one.
pub fn record(ledger: &Ledger, id: &str) -> Result<Option<Job>> {
    validate_id(id)?;
    store(ledger).read(&format!("{id}.json"))
}

/// What the record says about who may run the job.
pub fn ownership(ledger: &Ledger, id: &str) -> Result<Ownership> {
    let Some(job) = record(ledger, id)? else {
        return Ok(Ownership::Unclaimed);
    };
    Ok(if job.started_ms.is_some() {
        Ownership::Started(job)
    } else if job.ended_ms.is_some() {
        Ownership::Terminal(job)
    } else if job.state == PENDING {
        Ownership::Pending(job)
    } else {
        // Not executable: an intent without a pane, or a dispatch that was never proved.
        Ownership::Uncertain(job)
    })
}

/// Publish the durable intent for a launch that is about to allocate a pane. The intent is
/// not executable. It is written before the pane side effect, so an allocation that stops
/// here stays uncertain and cannot authorize another allocation.
pub fn publish_intent(ledger: &Ledger, job: &Job) -> Result<()> {
    validate_id(&job.id)?;
    store(ledger).with_lock(&job.id, || {
        if !matches!(ownership(ledger, &job.id)?, Ownership::Unclaimed) {
            return Err(Error::Invalid(format!(
                "job {} already has an owner",
                job.id
            )));
        }
        let mut intent = job.clone();
        intent.state = CLAIMING.into();
        intent.pane = String::new();
        intent.exit = None;
        intent.signal = None;
        intent.reason = None;
        intent.started_ms = None;
        intent.ended_ms = None;
        save(ledger, &intent)
    })
}

/// Publish the executable claim for `job`, once its pane is known. The intent must still be
/// the record's own: another owner never takes a job over.
pub fn publish_claim(ledger: &Ledger, job: &Job) -> Result<()> {
    validate_id(&job.id)?;
    store(ledger).with_lock(&job.id, || {
        match ownership(ledger, &job.id)? {
            Ownership::Uncertain(existing) if existing.state == CLAIMING => {}
            _ => {
                return Err(Error::Invalid(format!(
                    "job {} already has an owner",
                    job.id
                )));
            }
        }
        let mut claim = job.clone();
        claim.state = PENDING.into();
        claim.exit = None;
        claim.signal = None;
        claim.reason = None;
        claim.started_ms = None;
        claim.ended_ms = None;
        save(ledger, &claim)
    })
}

/// Keep the uncertainty of a dispatch that was not proved. A record the wrapper started or
/// ended is its evidence, and is never overwritten. Nothing here invents an end or a safe
/// retry: the job stays unproved until the host says otherwise.
pub fn uncertain(ledger: &Ledger, job: &Job, reason: String) -> Result<()> {
    validate_id(&job.id)?;
    store(ledger).with_lock(&job.id, || {
        let Some(current) = record(ledger, &job.id)? else {
            return Ok(());
        };
        if current.started_ms.is_some() || current.ended_ms.is_some() {
            return Ok(());
        }
        if current.state != PENDING {
            return Ok(());
        }
        let mut held = current;
        held.state = UNCERTAIN.into();
        held.reason = Some(reason);
        held.exit = None;
        held.signal = None;
        held.ended_ms = None;
        save(ledger, &held)
    })
}

/// The exact claim that authorizes this wrapper to run `job`. A record that is missing,
/// unreadable, for another command, already started, ended, or uncertain is refused here,
/// before any process, log, or pane exists.
fn pending_claim(ledger: &Ledger, job: &Job) -> Result<Job> {
    let Some(claimed) = record(ledger, &job.id)? else {
        return Err(Error::Invalid(format!(
            "job {} has no claim to run",
            job.id
        )));
    };
    if claimed.cmd != job.cmd {
        return Err(Error::Invalid(
            "exec command differs from the claimed command".into(),
        ));
    }
    if claimed.started_ms.is_some() {
        return Err(Error::Invalid("job already started".into()));
    }
    if claimed.ended_ms.is_some() {
        return Err(Error::Invalid("job already ended".into()));
    }
    if claimed.state != PENDING {
        return Err(Error::Invalid(format!(
            "job {} claim is not executable",
            job.id
        )));
    }
    Ok(claimed)
}

/// Mark the claim as started and return the running record. The short per-job record lock
/// holds this publication, and it is released before any capture.
fn start(ledger: &Ledger, job: &Job) -> Result<Job> {
    store(ledger).with_lock(&job.id, || {
        let mut running = pending_claim(ledger, job)?;
        running.started_ms = Some(now_ms());
        running.state = WORKING.into();
        save(ledger, &running)?;
        Ok(running)
    })
}

pub fn exec(ledger: &Ledger, shell: &dyn Shell, mut job: Job) -> Result<i32> {
    validate_id(&job.id)?;
    if job.cmd.is_empty() {
        return Err(Error::Invalid("empty command".into()));
    }
    // Exactly one wrapper may own an id for its entire lifetime.
    store(ledger).with_lock(&format!("{}-exec", job.id), || {
        // The claim is reloaded and validated under this lock, so a claim that a replay or
        // another wrapper changed cannot start a job. Nothing exists yet: no process, no log,
        // no pane.
        pending_claim(ledger, &job)?;
        job = start(ledger, &job)?;
        if let Err(error) = report(shell, &job, WORKING, None) {
            eprintln!("dogsled pane report: {error}");
        }
        let outcome = capture(ledger, &job);
        match &outcome {
            Ok((status, reason)) => {
                job.signal = status.signal();
                job.exit = Some(
                    if reason.as_deref().is_some_and(|r| r.ends_with("timeout")) {
                        124
                    } else {
                        status.code().unwrap_or(128 + status.signal().unwrap_or(0))
                    },
                );
                job.reason.clone_from(reason);
            }
            Err(e) => {
                job.exit = Some(2);
                job.reason = Some(e.to_string());
            }
        }
        let code = job.exit.unwrap_or(2);
        job.state = if code == 0 { "idle" } else { "blocked" }.into();
        job.ended_ms = Some(now_ms());
        save(ledger, &job)?;
        let elapsed = job
            .ended_ms
            .unwrap_or(0)
            .saturating_sub(job.started_ms.unwrap_or(0));
        let age = if elapsed >= 3_600_000 {
            format!("{}h", elapsed / 3_600_000)
        } else if elapsed >= 60_000 {
            format!("{}m", elapsed / 60_000)
        } else {
            format!("{}s", elapsed / 1_000)
        };
        let token = format!(
            "{} · {}",
            job.reason
                .as_ref()
                .map_or_else(|| format!("exit {code}"), Clone::clone),
            age
        );
        if let Err(error) = report(shell, &job, &job.state, Some(&token)) {
            eprintln!("dogsled pane report: {error}");
        }
        // Closing our own foreground pane can terminate this wrapper. Keep all
        // durable writes and completion reports above this final best-effort call.
        if !job.pane.is_empty()
            && let Err(error) =
                shell.run_ok("herdr", &["pane".into(), "close".into(), job.pane.clone()])
        {
            eprintln!("dogsled pane close: {error}");
        }
        outcome?;
        Ok(code)
    })
}
