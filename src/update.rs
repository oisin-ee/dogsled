//! Self-update, the way an ordinary app does it. A host reads the GitHub release feed,
//! compares the newest release with this binary, and does nothing when nothing is newer.
//!
//! The tool that installed the binary installs a newer release: the cargo-dist install
//! receipt that `dogsled-installer.sh` wrote, or the configured `[update] command`. When
//! neither applies, dogsled names the release and installs nothing.
//!
//! The harness plugins follow the binary. They update once per binary version, after the
//! binary is the newest release, and again on the next check when a step failed. Each
//! harness updates its own plugin with its own command; dogsled never edits their files.
//!
//! Herdr's running server keeps its version after a newer Herdr is installed, because a
//! restart ends every pane. Dogsled moves the server to the installed Herdr with Herdr's
//! live handoff, which keeps the panes and their processes running.

use crate::config::Update;
use crate::error::{Error, Result};
use crate::shell::Shell;
use axoupdater::{AxoUpdater, ReleaseSource, ReleaseSourceType, Version};
use serde::Serialize;
use std::path::Path;
use std::time::Duration;

const OWNER: &str = "oisin-ee";
const NAME: &str = "dogsled";
/// One installer or harness command. A harness update clones or downloads a release.
const STEP_LIMIT: Duration = Duration::from_secs(600);
/// The Pi packages this repository publishes.
const PI_PACKAGES: [&str; 5] = [
    "npm:@oisin-ee/pi-dogsled",
    "npm:@oisin-ee/oisinpi",
    "npm:@oisin-ee/pi-guardrails",
    "npm:@oisin-ee/pi-prewalk",
    "npm:@oisin-ee/pi-tiers",
];
/// The binary version that the harness plugins last updated to, in the ledger directory.
const PLUGINS_FILE: &str = "update-plugins";

/// What happened to the binary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "state", content = "version")]
pub enum Binary {
    /// This binary is the newest release, or no release has an installer yet.
    Current,
    /// A newer release exists. This run installs nothing.
    Available(String),
    /// A newer release exists, and no installer is known for this binary.
    Unmanaged(String),
    /// This run installed the release. The running process is still the old version.
    Installed(String),
}

/// What happened to one harness plugin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "state", content = "detail")]
pub enum Outcome {
    /// The harness ran its update command.
    Updated,
    /// The harness or the dogsled plugin is not installed, or the user pinned it.
    Absent,
    Failed(String),
}

#[derive(Debug, Clone, Serialize)]
pub struct Plugin {
    pub harness: &'static str,
    pub outcome: Outcome,
}

/// What happened to the running Herdr server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "state")]
pub enum Server {
    /// Herdr is not installed or not running, or the server runs the installed version or a
    /// newer one.
    Current,
    /// A newer Herdr is installed. This run changes nothing.
    Available { running: String, installed: String },
    /// A newer Herdr is installed, and a handoff cannot keep the running server's panes.
    Unsupported {
        running: String,
        installed: String,
        reason: String,
    },
    /// The server gave its panes to the installed Herdr. Their processes kept running.
    HandedOff { from: String, to: String },
    /// The handoff failed. Herdr keeps the old server and its panes.
    Failed { detail: String },
}

#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub current: String,
    /// The newest release with an installer, when the feed has one.
    pub latest: Option<String>,
    pub binary: Binary,
    pub plugins: Vec<Plugin>,
    pub herdr: Server,
}

impl Report {
    /// The process must exit so that its supervisor starts the new binary.
    #[must_use]
    pub const fn restart(&self) -> bool {
        matches!(self.binary, Binary::Installed(_))
    }

    #[must_use]
    pub fn lines(&self) -> String {
        let head = match &self.binary {
            Binary::Current => format!("dogsled {} is the newest release\n", self.current),
            Binary::Available(version) => format!("dogsled {version} is available\n"),
            Binary::Unmanaged(version) => format!(
                "dogsled {version} is available; update it with the tool that installed it\n"
            ),
            Binary::Installed(version) => format!("installed dogsled {version}\n"),
        };
        let plugins = self
            .plugins
            .iter()
            .filter_map(|plugin| match &plugin.outcome {
                Outcome::Updated => Some(format!("updated the {} plugin\n", plugin.harness)),
                Outcome::Absent => None,
                Outcome::Failed(error) => Some(format!("{} plugin: {error}\n", plugin.harness)),
            });
        let herdr = match &self.herdr {
            Server::Current => None,
            Server::Available { running, installed } => Some(format!(
                "herdr {installed} is installed; the server still runs {running}\n"
            )),
            Server::Unsupported {
                running,
                installed,
                reason,
            } => Some(format!(
                "herdr {installed} is installed; the {running} server stays, because {reason}\n"
            )),
            Server::HandedOff { from, to } => Some(format!(
                "moved the herdr server from {from} to {to}; its panes kept running\n"
            )),
            Server::Failed { detail } => Some(format!("herdr handoff: {detail}\n")),
        };
        std::iter::once(head).chain(plugins).chain(herdr).collect()
    }
}

/// The installer for a newer release, in order of precedence.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Installer {
    /// The configured command. The user chose it, so it wins over a receipt.
    Command(Vec<String>),
    /// The cargo-dist receipt is for this executable.
    Receipt,
    None,
}

fn installer(command: Option<&[String]>, receipt: bool) -> Installer {
    match command {
        Some(argv) if !argv.is_empty() => Installer::Command(argv.to_vec()),
        _ if receipt => Installer::Receipt,
        _ => Installer::None,
    }
}

/// The binary changes only for a strictly newer release; an older feed never downgrades.
fn newer<'a>(current: &Version, latest: Option<&'a Version>) -> Option<&'a Version> {
    latest.filter(|latest| *latest > current)
}

fn current() -> Result<Version> {
    Version::parse(env!("CARGO_PKG_VERSION")).map_err(|error| Error::Invalid(error.to_string()))
}

fn failed(error: &axoupdater::AxoupdateError) -> Error {
    Error::Invalid(format!("update: {error}"))
}

fn updater() -> AxoUpdater {
    let mut updater = AxoUpdater::new_for(NAME);
    updater.set_release_source(ReleaseSource {
        release_type: ReleaseSourceType::GitHub,
        owner: OWNER.into(),
        name: NAME.into(),
        app_name: NAME.into(),
    });
    updater.disable_installer_output();
    updater
}

/// The newest stable release that has an installer.
fn latest() -> Result<Option<Version>> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let mut updater = updater();
    runtime
        .block_on(updater.query_new_version())
        .map(Option::<&Version>::cloned)
        .map_err(|error| failed(&error))
}

/// The receipt's updater, when the receipt is for this executable.
fn receipt() -> Option<AxoUpdater> {
    let mut updater = updater();
    updater.load_receipt().ok()?;
    updater
        .check_receipt_is_for_this_executable()
        .ok()?
        .then_some(updater)
}

fn install(shell: &dyn Shell, settings: &Update, version: &Version) -> Result<Binary> {
    let mut receipt = receipt();
    match installer(settings.command.as_deref(), receipt.is_some()) {
        Installer::Command(argv) => {
            let argv: Vec<String> = argv
                .iter()
                .map(|arg| arg.replace("{version}", &version.to_string()))
                .collect();
            let (program, rest) = argv
                .split_first()
                .ok_or_else(|| Error::Invalid("update: empty command".into()))?;
            crate::shell::checked(program, rest, shell.run_bounded(program, rest, STEP_LIMIT)?)?;
        }
        Installer::Receipt => {
            // `None` means the receipt already names this release: another process installed it.
            // The file at the path is still newer than this process, so the result is the same.
            if let Some(updater) = receipt.as_mut() {
                updater.run_sync().map_err(|error| failed(&error))?;
            }
        }
        Installer::None => return Ok(Binary::Unmanaged(version.to_string())),
    }
    Ok(Binary::Installed(version.to_string()))
}

/// Check the feed, and when `apply` is set, install a newer release and update the plugins.
///
/// # Errors
/// The feed cannot be read, or the installer fails.
pub fn run(shell: &dyn Shell, settings: &Update, ledger: &Path, apply: bool) -> Result<Report> {
    // Herdr is checked first: it needs no network, so a feed error does not hold it back.
    let herdr = server(shell, apply);
    let current = current()?;
    let latest = latest()?;
    let binary = match newer(&current, latest.as_ref()) {
        Some(version) if apply => install(shell, settings, version)?,
        Some(version) => Binary::Available(version.to_string()),
        None => Binary::Current,
    };
    let target = match &binary {
        Binary::Installed(version) => Some(version.clone()),
        Binary::Current if latest.is_some() => Some(current.to_string()),
        _ => None,
    };
    let state = ledger.join(PLUGINS_FILE);
    let recorded = std::fs::read_to_string(&state).unwrap_or_default();
    let plugins = match target {
        Some(version) if apply && recorded.trim() != version => {
            let plugins = plugins(shell);
            if plugins
                .iter()
                .all(|p| !matches!(p.outcome, Outcome::Failed(_)))
            {
                std::fs::create_dir_all(ledger)?;
                std::fs::write(&state, &version)?;
            }
            plugins
        }
        _ => Vec::new(),
    };
    Ok(Report {
        current: current.to_string(),
        latest: latest.map(|version| version.to_string()),
        binary,
        plugins,
        herdr,
    })
}

/// The systemd unit in a `/proc/<pid>/cgroup` file, such as `herdr.service`.
fn unit(cgroup: &str) -> Option<&str> {
    cgroup
        .lines()
        .find_map(|line| line.strip_prefix("0::"))?
        .rsplit('/')
        .next()
        .filter(|unit| unit.ends_with(".service"))
}

/// Why a handoff would end the panes of the server at `socket`. A systemd unit with the
/// default `ExitType=main` stops when the old server exits, and systemd then kills the new
/// server and every pane. `ExitType=cgroup` keeps the unit active while the new server runs.
fn supervisor(shell: &dyn Shell, socket: &str) -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        use nix::sys::socket::{getsockopt, sockopt::PeerCredentials};
        let stream = std::os::unix::net::UnixStream::connect(socket).ok()?;
        let pid = getsockopt(&stream, PeerCredentials).ok()?.pid();
        let cgroup = std::fs::read_to_string(format!("/proc/{pid}/cgroup")).ok()?;
        let unit = unit(&cgroup)?;
        let exit = read(
            shell,
            "systemctl",
            &["--user", "show", "--property", "ExitType", "--value", unit],
        )
        .ok()
        .flatten()
        .unwrap_or_default();
        let exit = exit.trim();
        (exit != "cgroup").then(|| {
            format!("its systemd unit {unit} has ExitType={exit}, so a handoff would end its panes")
        })
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (shell, socket);
        None
    }
}

fn server(shell: &dyn Shell, apply: bool) -> Server {
    handoff(shell, apply).unwrap_or_else(|error| Server::Failed {
        detail: error.to_string(),
    })
}

/// Hand the running Herdr server to the installed Herdr, when the installed one is newer.
/// `herdr status client` describes the `herdr` on `PATH`; `herdr status server` describes
/// the running server. A failed handoff leaves the old server running with its panes.
fn handoff(shell: &dyn Shell, apply: bool) -> Result<Server> {
    let Some(client) = read(shell, "herdr", &["status", "client", "--json"])? else {
        return Ok(Server::Current);
    };
    let Some(status) = read(shell, "herdr", &["status", "server", "--json"])? else {
        return Ok(Server::Current);
    };
    let (client, status) = (json(&client)?, json(&status)?);
    let text = |value: &serde_json::Value| value.as_str().unwrap_or_default().to_owned();
    let (installed, running) = (text(&client["version"]), text(&status["version"]));
    let parse = |text: &str| Version::parse(text).ok();
    let newer = matches!(
        (parse(&installed), parse(&running)),
        (Some(installed), Some(running)) if installed > running
    );
    if status["running"] != true || !newer {
        return Ok(Server::Current);
    }
    let reason = if status["capabilities"]["live_handoff"] == true {
        supervisor(shell, status["socket"].as_str().unwrap_or_default())
    } else {
        Some("it cannot hand off its panes".to_owned())
    };
    if let Some(reason) = reason {
        return Ok(Server::Unsupported {
            running,
            installed,
            reason,
        });
    }
    if !apply {
        return Ok(Server::Available { running, installed });
    }
    let binary = text(&client["binary"]);
    let protocol = client["protocol"].to_string();
    step(
        shell,
        "herdr",
        &[
            "server",
            "live-handoff",
            "--import-exe",
            &binary,
            "--expected-version",
            &installed,
            "--expected-protocol",
            &protocol,
        ],
    )?;
    Ok(Server::HandedOff {
        from: running,
        to: installed,
    })
}

fn args(words: &[&str]) -> Vec<String> {
    words.iter().map(ToString::to_string).collect()
}

/// The command's stdout, or `None` when the harness is not installed.
fn read(shell: &dyn Shell, program: &str, words: &[&str]) -> Result<Option<String>> {
    match shell.run_bounded(program, &args(words), STEP_LIMIT) {
        Ok(out) if out.ok() => Ok(Some(out.stdout)),
        Ok(out) => Err(crate::shell::checked(program, &args(words), out)
            .err()
            .unwrap_or_else(|| Error::Invalid(format!("{program}: failed")))),
        Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn step(shell: &dyn Shell, program: &str, words: &[&str]) -> Result<()> {
    let argv = args(words);
    crate::shell::checked(
        program,
        &argv,
        shell.run_bounded(program, &argv, STEP_LIMIT)?,
    )
    .map(drop)
}

/// The unpinned Dogsled packages in `pi list`. A pinned package stays where the user put it.
fn pi_sources(list: &str) -> Vec<&str> {
    list.lines()
        .filter_map(|line| line.split_whitespace().next())
        .filter(|source| PI_PACKAGES.contains(source))
        .collect()
}

fn pi(shell: &dyn Shell) -> Result<bool> {
    let Some(list) = read(shell, "pi", &["list"])? else {
        return Ok(false);
    };
    let sources = pi_sources(&list);
    for source in &sources {
        step(shell, "pi", &["update", source])?;
    }
    Ok(!sources.is_empty())
}

fn json(text: &str) -> Result<serde_json::Value> {
    Ok(serde_json::from_str(text)?)
}

fn claude(shell: &dyn Shell) -> Result<bool> {
    let Some(list) = read(shell, "claude", &["plugin", "list", "--json"])? else {
        return Ok(false);
    };
    let installed = json(&list)?
        .as_array()
        .is_some_and(|plugins| plugins.iter().any(|p| p["id"] == "dogsled@dogsled"));
    if installed {
        step(
            shell,
            "claude",
            &["plugin", "marketplace", "update", "dogsled"],
        )?;
        step(shell, "claude", &["plugin", "update", "dogsled@dogsled"])?;
    }
    Ok(installed)
}

fn codex(shell: &dyn Shell) -> Result<bool> {
    let Some(list) = read(shell, "codex", &["plugin", "marketplace", "list", "--json"])? else {
        return Ok(false);
    };
    let installed = json(&list)?["marketplaces"]
        .as_array()
        .is_some_and(|all| all.iter().any(|m| m["name"] == "dogsled"));
    if installed {
        step(
            shell,
            "codex",
            &["plugin", "marketplace", "upgrade", "dogsled"],
        )?;
    }
    Ok(installed)
}

fn herdr(shell: &dyn Shell) -> Result<bool> {
    let Some(list) = read(
        shell,
        "herdr",
        &["plugin", "list", "--plugin", "dogsled", "--json"],
    )?
    else {
        return Ok(false);
    };
    // A linked plugin is a local checkout; its author updates it.
    let installed = json(&list)?["result"]["plugins"]
        .as_array()
        .is_some_and(|all| all.iter().any(|p| p["source"]["kind"] == "github"));
    if installed {
        step(shell, "herdr", &["plugin", "update", "dogsled", "--yes"])?;
    }
    Ok(installed)
}

/// A harness's update. `Ok(false)` means the harness or its dogsled plugin is absent.
type Harness = fn(&dyn Shell) -> Result<bool>;

fn plugins(shell: &dyn Shell) -> Vec<Plugin> {
    let harnesses: [(&'static str, Harness); 4] = [
        ("pi", pi),
        ("claude", claude),
        ("codex", codex),
        ("herdr", herdr),
    ];
    harnesses
        .into_iter()
        .map(|(harness, update)| Plugin {
            harness,
            outcome: match update(shell) {
                Ok(true) => Outcome::Updated,
                Ok(false) => Outcome::Absent,
                Err(error) => Outcome::Failed(error.to_string()),
            },
        })
        .collect()
}

/// How often a long-lived process looks for a newer executable.
pub const REPLACEMENT_CHECK: Duration = Duration::from_secs(30);

/// The file this process started from. An installer moves a new file over the path, so a
/// different file at the path is a newer build. The path is read at start: on Linux,
/// `current_exe` names a deleted file after the replacement.
#[derive(Debug, Clone)]
pub struct Executable {
    path: std::path::PathBuf,
    file: Option<(u64, u64)>,
}

fn identity(path: &Path) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt as _;
    std::fs::metadata(path).ok().map(|m| (m.dev(), m.ino()))
}

impl Executable {
    #[must_use]
    pub fn started() -> Self {
        let path = std::env::current_exe().unwrap_or_default();
        let file = identity(&path);
        Self { path, file }
    }

    /// A different file is at the start path.
    #[must_use]
    pub fn replaced(&self) -> bool {
        let now = identity(&self.path);
        now.is_some() && now != self.file
    }

    /// The newer build of this program, if one exists: the file at the start path, or the
    /// current installation of the Herdr plugin that this process came from. Herdr keeps an
    /// old installation for its running panes and installs an update in a new directory.
    #[must_use]
    pub fn successor(&self, shell: &dyn Shell) -> Option<Successor> {
        if self.replaced() {
            return Some(Successor {
                program: self.path.clone(),
                plugin_root: None,
            });
        }
        let root = std::path::PathBuf::from(std::env::var_os("HERDR_PLUGIN_ROOT")?);
        if !self.path.starts_with(&root) {
            return None;
        }
        let list = read(
            shell,
            "herdr",
            &["plugin", "list", "--plugin", NAME, "--json"],
        )
        .ok()??;
        let installed = json(&list).ok()?["result"]["plugins"][0]["plugin_root"]
            .as_str()
            .map(std::path::PathBuf::from)?;
        let program = installed.join(self.path.strip_prefix(&root).ok()?);
        (installed != root && program.is_file()).then_some(Successor {
            program,
            plugin_root: Some(installed),
        })
    }
}

/// The program that replaces this process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Successor {
    program: std::path::PathBuf,
    plugin_root: Option<std::path::PathBuf>,
}

impl Successor {
    /// Replace this process with the successor, with the same arguments. The terminal, the
    /// pane, and the process id stay. Returns only on failure.
    #[must_use]
    pub fn exec(&self) -> Error {
        use std::os::unix::process::CommandExt as _;
        let mut command = std::process::Command::new(&self.program);
        command.args(std::env::args_os().skip(1));
        if let Some(root) = &self.plugin_root {
            command.env("HERDR_PLUGIN_ROOT", root);
        }
        command.exec().into()
    }
}

/// `dogsled serve`'s update loop: check now and at each interval. In auto mode, a newer
/// release is installed and `restart` runs once, so the service can exit. Each distinct
/// report or error is printed once.
pub fn watch(settings: &Update, ledger: &Path, restart: &dyn Fn()) {
    let apply = settings.mode == crate::config::UpdateMode::Auto;
    let interval = Duration::from_secs(settings.interval_minutes.max(1) * 60);
    let shell = crate::shell::System::default();
    let mut last = String::new();
    loop {
        let (text, installed) = match run(&shell, settings, ledger, apply) {
            Ok(report) => (report.lines(), report.restart()),
            Err(error) => (format!("dogsled update: {error}\n"), false),
        };
        if text != last {
            eprint!("{text}");
            last = text;
        }
        if installed {
            restart();
            return;
        }
        std::thread::sleep(interval);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::Fake;

    fn version(text: &str) -> Version {
        Version::parse(text).unwrap()
    }

    #[test]
    fn only_a_strictly_newer_release_is_an_update() {
        let current = version("0.38.0");
        assert_eq!(
            newer(&current, Some(&version("0.39.0"))),
            Some(&version("0.39.0"))
        );
        assert_eq!(newer(&current, Some(&version("0.38.0"))), None);
        assert_eq!(newer(&current, Some(&version("0.37.2"))), None);
        assert_eq!(
            newer(&version("0.9.0"), Some(&version("0.10.0"))),
            Some(&version("0.10.0"))
        );
        assert_eq!(newer(&current, None), None);
    }

    #[test]
    fn a_configured_command_wins_then_the_receipt() {
        let command = vec!["mise".to_owned(), "use".to_owned()];
        assert_eq!(
            installer(Some(&command), true),
            Installer::Command(command.clone())
        );
        assert_eq!(installer(None, true), Installer::Receipt);
        assert_eq!(installer(Some(&[]), true), Installer::Receipt);
        assert_eq!(installer(None, false), Installer::None);
    }

    #[test]
    fn a_file_moved_over_the_path_is_a_replacement() {
        let dir = std::env::temp_dir().join(crate::ledger::new_id());
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("dogsled");
        std::fs::write(&path, "old").unwrap();
        let exe = Executable {
            file: identity(&path),
            path: path.clone(),
        };
        std::fs::write(&path, "same file, new bytes").unwrap();
        assert!(!exe.replaced());
        let next = dir.join("dogsled.new");
        std::fs::write(&next, "new").unwrap();
        std::fs::rename(&next, &path).unwrap();
        assert!(exe.replaced());
        assert_eq!(
            exe.successor(&Fake::new()),
            Some(Successor {
                program: path,
                plugin_root: None
            })
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn only_a_newer_installed_herdr_takes_over_the_running_server() {
        let client = r#"{"version":"0.9.6","protocol":23,"binary":"/i/herdr/0.9.6/herdr"}"#;
        let status = |version: &str, handoff: bool| {
            format!(
                r#"{{"running":true,"version":"{version}","capabilities":{{"live_handoff":{handoff}}}}}"#
            )
        };
        let handoff = "herdr server live-handoff --import-exe /i/herdr/0.9.6/herdr --expected-version 0.9.6 --expected-protocol 23";
        let check = |status: &str, apply: bool| {
            let shell = Fake::new();
            shell
                .on("herdr status client --json", client)
                .on("herdr status server --json", status)
                .on(handoff, "live handoff complete");
            let outcome = server(&shell, apply);
            let ran = shell.calls.lock().unwrap().iter().any(|c| c == handoff);
            (outcome, ran)
        };
        let versions = |running: &str| (running.to_owned(), "0.9.6".to_owned());
        let (running, installed) = versions("0.9.5");
        assert_eq!(
            check(&status("0.9.5", true), true),
            (
                Server::HandedOff {
                    from: running.clone(),
                    to: installed.clone()
                },
                true
            )
        );
        assert_eq!(
            check(&status("0.9.5", true), false),
            (
                Server::Available {
                    running: running.clone(),
                    installed: installed.clone()
                },
                false
            )
        );
        assert_eq!(
            check(&status("0.9.5", false), true),
            (
                Server::Unsupported {
                    running,
                    installed,
                    reason: "it cannot hand off its panes".to_owned()
                },
                false
            )
        );
        assert_eq!(
            check(&status("0.9.6", true), true),
            (Server::Current, false)
        );
        assert_eq!(
            check(&status("0.9.7", true), true),
            (Server::Current, false)
        );
        assert_eq!(
            check(r#"{"running":false,"version":null}"#, true),
            (Server::Current, false)
        );
    }

    #[test]
    fn the_server_unit_comes_from_its_cgroup() {
        let service = "0::/user.slice/user-1001.slice/user@1001.service/app.slice/herdr.service\n";
        assert_eq!(unit(service), Some("herdr.service"));
        assert_eq!(
            unit("0::/user.slice/user-501.slice/session-3.scope\n"),
            None
        );
    }

    #[test]
    fn only_unpinned_dogsled_pi_packages_update() {
        let list = "User packages:\n  npm:@oisin-ee/pi-dogsled\n    /x\n  npm:@oisin-ee/pi-tiers@0.38.0\n    /y\n  npm:@other/pi-dogsled\n  npm:@oisin-ee/pi-prewalk (filtered)\n";
        assert_eq!(
            pi_sources(list),
            ["npm:@oisin-ee/pi-dogsled", "npm:@oisin-ee/pi-prewalk"]
        );
    }

    #[test]
    fn each_installed_harness_runs_its_own_update_command() {
        let shell = Fake::new();
        shell
            .on("pi list", "User packages:\n  npm:@oisin-ee/pi-dogsled\n")
            .on("pi update npm:@oisin-ee/pi-dogsled", "")
            .on("claude plugin list --json", r#"[{"id":"dogsled@dogsled"}]"#)
            .on("claude plugin marketplace update dogsled", "")
            .on("claude plugin update dogsled@dogsled", "")
            .on(
                "codex plugin marketplace list --json",
                r#"{"marketplaces":[{"name":"other"}]}"#,
            )
            .on(
                "herdr plugin list --plugin dogsled --json",
                r#"{"result":{"plugins":[{"source":{"kind":"github"}}]}}"#,
            )
            .on("herdr plugin update dogsled --yes", "");
        let outcomes: Vec<_> = plugins(&shell)
            .into_iter()
            .map(|p| (p.harness, p.outcome))
            .collect();
        assert_eq!(
            outcomes,
            [
                ("pi", Outcome::Updated),
                ("claude", Outcome::Updated),
                ("codex", Outcome::Absent),
                ("herdr", Outcome::Updated),
            ]
        );
    }
}
