//! The one seam between the engine and the operating system. Tests supply a
//! fake; the binary supplies `System`.

use std::process::{Command, Stdio};

use crate::error::{Error, Result};

/// Captured result of a finished process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Output {
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
}

impl Output {
    #[must_use]
    pub fn ok(&self) -> bool {
        self.status == 0
    }
}

pub trait Shell: Send + Sync + std::fmt::Debug {
    fn budget(&self) -> crate::budget::Budget {
        crate::budget::Budget::default()
    }
    /// Run `program args…` with no stdin, capturing both streams. Returns
    /// `Err` only when the process could not be started.
    fn run(&self, program: &str, args: &[String]) -> Result<Output>;

    /// Run with a caller-owned wall-clock budget.
    fn run_bounded(
        &self,
        program: &str,
        args: &[String],
        timeout: std::time::Duration,
    ) -> Result<Output> {
        let _ = timeout;
        self.run(program, args)
    }

    /// Input is never part of argv or diagnostics. Implementations must bound writing and
    /// waiting together, and must not log the input.
    fn run_input_bounded(
        &self,
        program: &str,
        args: &[String],
        input: &[u8],
        timeout: std::time::Duration,
    ) -> Result<Output> {
        let _ = (program, args, input, timeout);
        Err(Error::Invalid("shell does not support stdin input".into()))
    }

    /// Setup has a separate ceiling; the command deadline still takes precedence.
    fn run_setup(&self, program: &str, args: &[String]) -> Result<Output> {
        self.run_bounded(
            program,
            args,
            self.budget()
                .with_limit(program, crate::budget::SETUP_LIMIT)?,
        )
    }

    fn run_setup_ok(&self, program: &str, args: &[String]) -> Result<Output> {
        checked(program, args, self.run_setup(program, args)?)
    }

    /// Read a whole file (for `/proc/loadavg`). Fakes script this too so the
    /// probe tests run the same on every platform.
    fn read_file(&self, path: &str) -> Result<String> {
        Ok(std::fs::read_to_string(path)?)
    }

    /// Run and treat a non-zero status as an error.
    fn run_ok(&self, program: &str, args: &[String]) -> Result<Output> {
        checked(program, args, self.run(program, args)?)
    }
}

pub(crate) fn checked(program: &str, args: &[String], out: Output) -> Result<Output> {
    if out.ok() {
        Ok(out)
    } else {
        Err(Error::Command {
            program: program.to_string(),
            args: args.join(" "),
            status: out.status.to_string(),
            stderr: out.stderr.trim().to_string(),
        })
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct System {
    pub budget: crate::budget::Budget,
}

impl Shell for System {
    fn budget(&self) -> crate::budget::Budget {
        self.budget
    }
    fn run_bounded(
        &self,
        program: &str,
        args: &[String],
        timeout: std::time::Duration,
    ) -> Result<Output> {
        self.execute(program, args, None, timeout)
    }

    fn run_input_bounded(
        &self,
        program: &str,
        args: &[String],
        input: &[u8],
        timeout: std::time::Duration,
    ) -> Result<Output> {
        self.execute(program, args, Some(input), timeout)
    }

    fn run(&self, program: &str, args: &[String]) -> Result<Output> {
        self.run_bounded(program, args, self.budget.remaining(program)?)
    }
}

impl System {
    fn execute(
        &self,
        program: &str,
        args: &[String],
        input: Option<&[u8]>,
        timeout: std::time::Duration,
    ) -> Result<Output> {
        use std::io::Write as _;
        use std::os::unix::process::CommandExt as _;
        let operation = crate::diagnostic::shell_operation(program).to_owned();
        let timeout = self.budget.with_limit(&operation, timeout)?;
        if timeout.is_zero() {
            return Err(Error::Deadline(operation));
        }
        let mut child = Command::new(program)
            .args(args)
            .stdin(if input.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .spawn()?;
        let pid = child.id();
        let writer = input.map(|input| {
            let input = input.to_vec();
            let stdin = child.stdin.take();
            std::thread::spawn(move || -> std::io::Result<()> {
                if let Some(mut stdin) = stdin {
                    stdin.write_all(&input)?;
                }
                Ok(())
            })
        });
        let (send, recv) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let output = child.wait_with_output();
            let written = writer.map(std::thread::JoinHandle::join);
            let result = output.and_then(|output| {
                if let Some(written) = written {
                    written.map_err(|_| std::io::Error::other("stdin worker failed"))??;
                }
                Ok(output)
            });
            let _ = send.send(result);
        });
        let result = match recv.recv_timeout(timeout) {
            Ok(result) => result.map_err(Error::from),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                let deadline = Error::Deadline(operation);
                if let Err(error) = kill_group(pid) {
                    // Do not join a worker whose process may still be running. Keep
                    // cleanup failures distinct from an ordinary waiting deadline.
                    return Err(Error::Invalid(format!("{deadline}; {error}")));
                }
                Err(deadline)
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                Err(Error::Invalid("command worker disconnected".into()))
            }
        };
        worker
            .join()
            .map_err(|_| Error::Invalid("command worker failed".into()))?;
        let out = result?;
        Ok(Output {
            status: out.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        })
    }
}

/// Kill an owned command group, accepting a group that has already disappeared.
pub(crate) fn kill_group(pid: u32) -> Result<()> {
    kill_group_with(pid, nix::sys::signal::killpg)
}

fn kill_group_with(
    pid: u32,
    mut signal: impl FnMut(nix::unistd::Pid, Option<nix::sys::signal::Signal>) -> nix::Result<()>,
) -> Result<()> {
    use nix::errno::Errno;
    use nix::sys::signal::Signal;
    use nix::unistd::Pid;

    let pid = i32::try_from(pid).map_err(|e| Error::Invalid(e.to_string()))?;
    let group = Pid::from_raw(pid);
    match signal(group, Some(Signal::SIGKILL)) {
        Ok(()) | Err(Errno::ESRCH) => Ok(()),
        // Darwin can find the group but lose its last live member during iteration,
        // returning EPERM instead of ESRCH. A signal-free probe must confirm the
        // whole group is gone; the leader's exit alone says nothing about descendants.
        Err(Errno::EPERM) if signal(group, None) == Err(Errno::ESRCH) => Ok(()),
        Err(error) => Err(Error::Invalid(format!("kill process group {pid}: {error}"))),
    }
}

/// Scripted shell for tests: a map from `"program arg1 arg2"` to the output
/// to return, plus a log of every call. Scripts may change after the fake
/// has been lent out, so both maps sit behind a mutex.
#[cfg(test)]
#[derive(Debug, Default)]
pub struct Fake {
    responses: std::sync::Mutex<std::collections::HashMap<String, Output>>,
    pub calls: std::sync::Mutex<Vec<String>>,
    /// Each bounded call and the time limit it was given.
    pub limits: std::sync::Mutex<Vec<(String, std::time::Duration)>>,
}

#[cfg(test)]
impl Fake {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn on(&self, command: &str, stdout: &str) -> &Self {
        self.on_output(
            command,
            Output {
                status: 0,
                stdout: stdout.to_string(),
                stderr: String::new(),
            },
        )
    }

    pub fn on_output(&self, command: &str, output: Output) -> &Self {
        if let Ok(mut r) = self.responses.lock() {
            r.insert(command.to_string(), output);
        }
        self
    }
}

#[cfg(test)]
impl Shell for Fake {
    fn run_bounded(
        &self,
        program: &str,
        args: &[String],
        timeout: std::time::Duration,
    ) -> Result<Output> {
        if let Ok(mut limits) = self.limits.lock() {
            limits.push((format!("{program} {}", args.join(" ")), timeout));
        }
        self.run(program, args)
    }

    fn run_input_bounded(
        &self,
        program: &str,
        args: &[String],
        _input: &[u8],
        timeout: std::time::Duration,
    ) -> Result<Output> {
        self.run_bounded(program, args, timeout)
    }

    /// Files are scripted under the key `read <path>`.
    fn read_file(&self, path: &str) -> Result<String> {
        self.run("read", &[path.to_string()]).map(|o| o.stdout)
    }

    fn run(&self, program: &str, args: &[String]) -> Result<Output> {
        let key = std::iter::once(program.to_string())
            .chain(args.iter().cloned())
            .collect::<Vec<_>>()
            .join(" ");
        if let Ok(mut calls) = self.calls.lock() {
            calls.push(key.clone());
        }
        let scripted = self
            .responses
            .lock()
            .ok()
            .and_then(|r| r.get(&key).cloned());
        scripted.ok_or_else(|| Error::Command {
            program: program.to_string(),
            args: args.join(" "),
            status: "fake".to_string(),
            stderr: format!("no scripted response for `{key}`"),
        })
    }
}

/// Single-quote one argument for a POSIX shell on the far side of ssh.
#[must_use]
pub fn quote(arg: &str) -> String {
    if !arg.is_empty()
        && arg
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_./:=@%+,".contains(&b))
    {
        return arg.to_string();
    }
    format!("'{}'", arg.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deadline_never_formats_scripts_credentials_or_launch_json() {
        let shell = System::default();
        let error = shell
            .run_bounded(
                "sh",
                &[
                    "-c".into(),
                    "sleep 1; echo TASK_SECRET https://user:TOKEN_SECRET@host".into(),
                ],
                std::time::Duration::from_millis(10),
            )
            .unwrap_err();
        assert!(matches!(error.root(), Error::Deadline(_)));
        for sink in [error.to_string(), format!("{error:?}")] {
            assert!(!sink.contains("TASK_SECRET"), "{sink}");
            assert!(!sink.contains("TOKEN_SECRET"), "{sink}");
        }
    }

    #[test]
    fn kill_group_accepts_success_and_absent_groups_without_probing() {
        use nix::errno::Errno;
        use nix::sys::signal::Signal;

        for outcome in [Ok(()), Err(Errno::ESRCH)] {
            let mut calls = Vec::new();
            assert!(
                kill_group_with(42, |group, signal| {
                    calls.push((group.as_raw(), signal));
                    outcome
                })
                .is_ok()
            );
            assert_eq!(calls, [(42, Some(Signal::SIGKILL))]);
        }
    }

    #[test]
    fn kill_group_accepts_eperm_only_when_the_group_is_confirmed_absent() {
        use nix::errno::Errno;
        use nix::sys::signal::Signal;

        for probe in [
            Err(Errno::ESRCH),
            Ok(()),
            Err(Errno::EPERM),
            Err(Errno::EIO),
        ] {
            let mut calls = Vec::new();
            let result = kill_group_with(42, |group, signal| {
                calls.push((group.as_raw(), signal));
                if signal.is_some() {
                    Err(Errno::EPERM)
                } else {
                    probe
                }
            });
            assert_eq!(calls, [(42, Some(Signal::SIGKILL)), (42, None)]);
            if probe == Err(Errno::ESRCH) {
                assert!(result.is_ok());
            } else {
                assert!(matches!(result, Err(Error::Invalid(message))
                    if message == format!("kill process group 42: {}", Errno::EPERM)));
            }
        }
    }

    #[test]
    fn kill_group_reports_other_errors_without_probing() {
        use nix::errno::Errno;
        use nix::sys::signal::Signal;

        let mut calls = Vec::new();
        let result = kill_group_with(42, |group, signal| {
            calls.push((group.as_raw(), signal));
            Err(Errno::EINVAL)
        });
        assert_eq!(calls, [(42, Some(Signal::SIGKILL))]);
        assert!(matches!(result, Err(Error::Invalid(message))
            if message == format!("kill process group 42: {}", Errno::EINVAL)));
    }

    #[test]
    fn quote_leaves_safe_words_alone() {
        assert_eq!(quote("w-abc-12"), "w-abc-12");
        assert_eq!(quote("--ttl-ms=5"), "--ttl-ms=5");
    }

    #[test]
    fn quote_wraps_and_escapes() {
        assert_eq!(quote("a b"), "'a b'");
        assert_eq!(quote("it's"), "'it'\\''s'");
        assert_eq!(quote(""), "''");
    }
}
