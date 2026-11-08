//! The one seam between the engine and the operating system. Tests supply a
//! fake; the binary supplies `System`.

use std::process::{Command, Stdio};

/// Text commands must admit the largest legitimate reader: an SSH peer reply whose
/// body is bounded by the detail codec, plus its status/body JSON envelope and newline.
pub(crate) const OUTPUT_LIMIT: usize = crate::observation::details::CODEC.wire_limit() + 64;

use crate::error::{Error, Result};

/// Repository-local variables from `git rev-parse --local-env-vars`, plus namespace
/// routing. Do not inherit a hook's repository, index, object store, or local config.
/// Intentional overrides must be supplied explicitly, for example through `env` argv.
pub(crate) const GIT_LOCAL_ENV: &[&str] = &[
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_CONFIG",
    "GIT_CONFIG_PARAMETERS",
    "GIT_CONFIG_COUNT",
    "GIT_OBJECT_DIRECTORY",
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_IMPLICIT_WORK_TREE",
    "GIT_GRAFT_FILE",
    "GIT_INDEX_FILE",
    "GIT_NO_REPLACE_OBJECTS",
    "GIT_REPLACE_REF_BASE",
    "GIT_PREFIX",
    "GIT_SHALLOW_FILE",
    "GIT_COMMON_DIR",
    "GIT_NAMESPACE",
];

/// A command for a dogsled child process. Every child starts here, so none inherits a
/// hook's repository routing.
pub(crate) fn command(program: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut command = Command::new(program);
    for key in GIT_LOCAL_ENV {
        command.env_remove(key);
    }
    command
}

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

/// Byte-preserving captured stdout, using the same process budget as text commands.
#[derive(Debug)]
pub struct BytesOutput {
    pub status: i32,
    pub stdout: Vec<u8>,
    pub stderr: String,
}

pub trait Shell: Send + Sync + std::fmt::Debug {
    fn run_bytes_bounded(
        &self,
        program: &str,
        args: &[String],
        timeout: std::time::Duration,
        limit: usize,
    ) -> Result<BytesOutput> {
        let _ = (program, args, timeout, limit);
        Err(Error::Invalid("shell does not support byte output".into()))
    }
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

    /// Stdin input with raw stdout, for batch protocols such as `git cat-file --batch`.
    fn run_input_bytes_bounded(
        &self,
        program: &str,
        args: &[String],
        input: &[u8],
        timeout: std::time::Duration,
        limit: usize,
    ) -> Result<BytesOutput> {
        let _ = (program, args, input, timeout, limit);
        Err(Error::Invalid("shell does not support byte output".into()))
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
    fn run_bytes_bounded(
        &self,
        program: &str,
        args: &[String],
        timeout: std::time::Duration,
        limit: usize,
    ) -> Result<BytesOutput> {
        self.execute_bytes(program, args, None, timeout, limit)
    }
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

    fn run_input_bytes_bounded(
        &self,
        program: &str,
        args: &[String],
        input: &[u8],
        timeout: std::time::Duration,
        limit: usize,
    ) -> Result<BytesOutput> {
        self.execute_bytes(program, args, Some(input), timeout, limit)
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
        let out = self.execute_bytes(program, args, input, timeout, OUTPUT_LIMIT)?;
        Ok(Output {
            status: out.status,
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: out.stderr,
        })
    }

    fn execute_bytes(
        &self,
        program: &str,
        args: &[String],
        input: Option<&[u8]>,
        timeout: std::time::Duration,
        limit: usize,
    ) -> Result<BytesOutput> {
        use std::io::Write as _;
        use std::os::unix::process::CommandExt as _;
        let operation = crate::diagnostic::shell_operation(program).to_owned();
        let timeout = self.budget.with_limit(&operation, timeout)?;
        if timeout.is_zero() {
            return Err(Error::Deadline(operation));
        }
        let mut child = command(program)
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
        let capture_operation = operation.clone();
        let worker = std::thread::spawn(move || {
            let result = capture(child, writer, limit, capture_operation);
            let _ = send.send(result);
        });
        let result = match recv.recv_timeout(timeout) {
            Ok(result) => result,
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
        result
    }
}

/// Drain stderr concurrently while reading at most one byte beyond the stdout bound.
fn capture(
    mut child: std::process::Child,
    writer: Option<std::thread::JoinHandle<std::io::Result<()>>>,
    limit: usize,
    operation: String,
) -> Result<BytesOutput> {
    use std::io::Read as _;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let errors = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Some(mut stderr) = stderr {
            stderr.read_to_end(&mut bytes)?;
        }
        Ok::<_, std::io::Error>(bytes)
    });
    let mut bytes = Vec::new();
    let read = stdout.map_or(Ok(0), |stdout| {
        stdout
            .take(u64::try_from(limit.saturating_add(1)).unwrap_or(u64::MAX))
            .read_to_end(&mut bytes)
    });
    let exceeded = bytes.len() > limit;
    if exceeded || read.is_err() {
        kill_child_group(&mut child)?;
    }
    let status = child.wait();
    let stderr = errors
        .join()
        .map_err(|_| Error::Invalid("stderr worker failed".into()))?;
    let written = writer.map(std::thread::JoinHandle::join);
    if exceeded {
        return Err(Error::OutputLimit { operation, limit });
    }
    read?;
    let status = status?;
    if let Some(written) = written {
        written.map_err(|_| Error::Invalid("stdin worker failed".into()))??;
    }
    Ok(BytesOutput {
        status: status.code().unwrap_or(-1),
        stdout: bytes,
        stderr: String::from_utf8_lossy(&stderr?).into_owned(),
    })
}

/// Kill our own child's group. Darwin answers EPERM while the group's only member is an
/// exiting leader, because a zombie cannot be signalled. The leader is our child, so reap it;
/// the group must then be gone or signalable.
fn kill_child_group(child: &mut std::process::Child) -> Result<()> {
    let mut refused = false;
    let killed = kill_group_with(child.id(), |group, signal| {
        let result = nix::sys::signal::killpg(group, signal);
        refused |= result == Err(nix::errno::Errno::EPERM);
        result
    });
    match killed {
        Err(_) if refused => {
            child.wait()?;
            kill_group(child.id())
        }
        other => other,
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
    fn run_bytes_bounded(
        &self,
        program: &str,
        args: &[String],
        timeout: std::time::Duration,
        limit: usize,
    ) -> Result<BytesOutput> {
        let out = self.run_bounded(program, args, timeout)?;
        if out.stdout.len() > limit {
            return Err(Error::OutputLimit {
                operation: crate::diagnostic::shell_operation(program).into(),
                limit,
            });
        }
        Ok(BytesOutput {
            status: out.status,
            stdout: out.stdout.into_bytes(),
            stderr: out.stderr,
        })
    }

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

    struct GitFixture(std::path::PathBuf);

    impl GitFixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(crate::ledger::new_id());
            std::fs::create_dir_all(&root).unwrap();
            let fixture = Self(root.canonicalize().unwrap());
            for name in ["repo", "hook"] {
                Self::git(&["init", "-q", "-b", "main", &fixture.path(name)]);
                std::fs::write(fixture.0.join(name).join("file"), name).unwrap();
                Self::git(&["-C", &fixture.path(name), "add", "."]);
                Self::git(&["-C", &fixture.path(name), "commit", "-qm", "base"]);
            }
            Self::git(&[
                "-C",
                &fixture.path("repo"),
                "worktree",
                "add",
                "-qb",
                "feature",
                &fixture.path("tree"),
            ]);
            std::fs::write(fixture.0.join("tree/feature"), "feature\n").unwrap();
            Self::git(&["-C", &fixture.path("tree"), "add", "."]);
            Self::git(&["-C", &fixture.path("tree"), "commit", "-qm", "feature"]);
            fixture
        }

        fn path(&self, name: &str) -> String {
            self.0.join(name).display().to_string()
        }

        fn git(args: &[&str]) -> String {
            let mut words = Vec::new();
            for key in GIT_LOCAL_ENV {
                words.extend(["-u".into(), (*key).into()]);
            }
            words.extend([
                "git".into(),
                "-c".into(),
                "user.name=Fixture".into(),
                "-c".into(),
                "user.email=fixture@example.invalid".into(),
            ]);
            words.extend(args.iter().map(ToString::to_string));
            System::default().run_ok("env", &words).unwrap().stdout
        }
    }

    impl Drop for GitFixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn with_hook_environment(test: &str, check: impl FnOnce(&GitFixture)) {
        const ROOT: &str = "DOGSLED_SHELL_GIT_FIXTURE";
        if let Some(root) = std::env::var_os(ROOT) {
            // The parent also removes this directory if the child times out.
            let fixture = GitFixture(std::path::PathBuf::from(root));
            check(&fixture);
            return;
        }
        let fixture = GitFixture::new();
        let mut words = GIT_LOCAL_ENV
            .iter()
            .map(|key| format!("{key}=hook-local"))
            .collect::<Vec<_>>();
        words.extend([
            format!("{ROOT}={}", fixture.0.display()),
            format!("GIT_DIR={}/hook/.git", fixture.0.display()),
            format!("GIT_WORK_TREE={}/hook", fixture.0.display()),
            format!("GIT_INDEX_FILE={}/hook/.git/index", fixture.0.display()),
            "GIT_SSH_COMMAND=retained-transport-setting".into(),
            std::env::current_exe().unwrap().display().to_string(),
            "--exact".into(),
            format!("shell::tests::{test}"),
            "--nocapture".into(),
        ]);
        let output = System::default()
            .run_bounded("env", &words, std::time::Duration::from_secs(30))
            .unwrap();
        assert!(output.ok(), "{}\n{}", output.stdout, output.stderr);
    }

    #[test]
    fn system_observes_requested_worktree_inside_hook_environment() {
        with_hook_environment(
            "system_observes_requested_worktree_inside_hook_environment",
            |fixture| {
                let shell = System::default();
                let tree = fixture.0.join("tree");
                let facts = crate::facts::worktree(&shell, tree.to_str().unwrap(), None).unwrap();
                assert_eq!(facts.path, tree.to_str().unwrap());
                assert_eq!(facts.branch, "feature");
                assert_eq!(facts.ahead, 1);
                assert!(!facts.dirty);
                assert!(!facts.landed);
                assert_eq!(
                    facts.head,
                    GitFixture::git(&["-C", tree.to_str().unwrap(), "rev-parse", "HEAD"]).trim()
                );
                for output in [
                    shell.run_ok("env", &[]).unwrap().stdout.into_bytes(),
                    shell
                        .run_bytes_bounded(
                            "env",
                            &[],
                            std::time::Duration::from_secs(2),
                            OUTPUT_LIMIT,
                        )
                        .unwrap()
                        .stdout,
                    shell
                        .run_input_bounded(
                            "sh",
                            &["-c".into(), "cat >/dev/null; env".into()],
                            b"input",
                            std::time::Duration::from_secs(2),
                        )
                        .unwrap()
                        .stdout
                        .into_bytes(),
                ] {
                    let output = String::from_utf8(output).unwrap();
                    assert!(
                        output
                            .lines()
                            .any(|line| line == "GIT_SSH_COMMAND=retained-transport-setting")
                    );
                    assert!(output.lines().any(|line| line.starts_with("PATH=")));
                    for key in GIT_LOCAL_ENV {
                        assert!(
                            !output
                                .lines()
                                .any(|line| line.starts_with(&format!("{key}="))),
                            "{key}"
                        );
                    }
                }
                std::fs::write(tree.join("feature"), "dirty\n").unwrap();
                assert!(
                    crate::facts::worktree(&shell, tree.to_str().unwrap(), Some(&facts))
                        .unwrap()
                        .dirty
                );
            },
        );
    }

    #[test]
    fn preservation_temporary_index_overrides_ambient_hook_index() {
        with_hook_environment(
            "preservation_temporary_index_overrides_ambient_hook_index",
            |fixture| {
                let shell = System::default();
                let repo = fixture.0.join("repo");
                let index = repo.join(".git/index");
                let hook_index = fixture.0.join("hook/.git/index");
                std::fs::write(repo.join("file"), "staged\n").unwrap();
                GitFixture::git(&["-C", repo.to_str().unwrap(), "add", "file"]);
                std::fs::write(repo.join("file"), "working\n").unwrap();
                let before = std::fs::read(&index).unwrap();
                let hook_before = std::fs::read(&hook_index).unwrap();
                let saved = crate::preservation::snapshot(&shell, &repo).unwrap();
                for (tree, expected) in [
                    (&saved.index_tree, "staged\n"),
                    (&saved.worktree_tree, "working\n"),
                ] {
                    let out = shell
                        .run_ok(
                            "git",
                            &[
                                "-C".into(),
                                repo.display().to_string(),
                                "cat-file".into(),
                                "blob".into(),
                                format!("{tree}:file"),
                            ],
                        )
                        .unwrap();
                    assert_eq!(out.stdout, expected);
                }
                assert_ne!(saved.index_tree, saved.worktree_tree);
                assert_eq!(std::fs::read(index).unwrap(), before);
                assert_eq!(std::fs::read(hook_index).unwrap(), hook_before);
            },
        );
    }

    #[test]
    fn every_git_local_variable_is_isolated() {
        let reported = System::default()
            .run_ok("git", &["rev-parse".into(), "--local-env-vars".into()])
            .unwrap()
            .stdout;
        let reported = reported.lines().collect::<Vec<_>>();
        assert!(reported.contains(&"GIT_DIR"), "{reported:?}");
        for key in reported {
            assert!(GIT_LOCAL_ENV.contains(&key), "{key} is not isolated");
        }
    }

    #[test]
    fn byte_output_preserves_non_utf8_and_obeys_deadlines() {
        let shell = System::default();
        let output = shell
            .run_bytes_bounded(
                "sh",
                &["-c".into(), "printf '\\000\\377'".into()],
                std::time::Duration::from_secs(2),
                2,
            )
            .unwrap();
        assert_eq!(output.status, 0);
        assert_eq!(output.stdout, [0, 255]);
        let error = shell
            .run_bytes_bounded(
                "sh",
                &["-c".into(), "sleep 1".into()],
                std::time::Duration::from_millis(10),
                2,
            )
            .unwrap_err();
        assert!(matches!(error.root(), Error::Deadline(_)));
    }

    #[test]
    fn stdout_limit_kills_the_owned_group_before_natural_exit() {
        let root = std::env::temp_dir().join(crate::ledger::new_id());
        std::fs::create_dir_all(&root).unwrap();
        // The descendant would outlive the call's deadline, so an output-limit error proves
        // the call returned before any natural exit, however slowly the host spawns.
        let pidfile = root.join("descendant");
        let script = format!(
            "sleep 30 & echo $! > {}; printf 123456789; wait",
            quote(pidfile.to_str().unwrap())
        );
        let error = System::default()
            .run_bytes_bounded(
                "sh",
                &["-c".into(), script],
                std::time::Duration::from_secs(5),
                8,
            )
            .unwrap_err();
        assert!(
            matches!(error, Error::OutputLimit { limit: 8, .. }),
            "{error:?}"
        );
        // The group kill reached the descendant. Its new parent reaps it on its own
        // schedule, so wait for the pid to vanish on a bound.
        let pid: i32 = std::fs::read_to_string(&pidfile)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        let pid = nix::unistd::Pid::from_raw(pid);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while nix::sys::signal::kill(pid, None) != Err(nix::errno::Errno::ESRCH) {
            assert!(
                std::time::Instant::now() < deadline,
                "descendant {pid} survived stdout overflow"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn fake_enforces_the_same_stdout_bound() {
        let shell = Fake::new();
        shell.on("git status", "12345");
        for limit in [0, 4, 5] {
            let result = shell.run_bytes_bounded(
                "git",
                &["status".into()],
                std::time::Duration::from_secs(1),
                limit,
            );
            if limit < 5 {
                assert!(matches!(result, Err(Error::OutputLimit { .. })));
            } else {
                assert_eq!(result.unwrap().stdout, b"12345");
            }
        }
    }

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
    fn kill_child_group_reaps_an_exited_leader() {
        use std::os::unix::process::CommandExt as _;
        let mut child = std::process::Command::new("true")
            .process_group(0)
            .spawn()
            .unwrap();
        // Wait, without reaping, until the leader is a zombie. Its group then still exists:
        // Darwin refuses to signal it (EPERM) and Linux accepts.
        let pid = child.id().to_string();
        while !String::from_utf8_lossy(
            &std::process::Command::new("ps")
                .args(["-o", "stat=", "-p", &pid])
                .output()
                .unwrap()
                .stdout,
        )
        .trim_start()
        .starts_with('Z')
        {}
        kill_child_group(&mut child).unwrap();
        assert!(child.try_wait().unwrap().is_some());
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
