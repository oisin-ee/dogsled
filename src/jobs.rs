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

pub fn exec(ledger: &Ledger, shell: &dyn Shell, mut job: Job) -> Result<i32> {
    validate_id(&job.id)?;
    if job.cmd.is_empty() {
        return Err(Error::Invalid("empty command".into()));
    }
    // Exactly one wrapper may own an id for its entire lifetime.
    store(ledger).with_lock(&format!("{}-exec", job.id), || {
        if get(ledger, &job.id).is_ok_and(|j| j.started_ms.is_some()) {
            return Err(Error::Invalid("job already started".into()));
        }
        job.started_ms = Some(now_ms());
        job.state = "working".into();
        save(ledger, &job)?;
        if let Err(error) = report(shell, &job, "working", None) {
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
