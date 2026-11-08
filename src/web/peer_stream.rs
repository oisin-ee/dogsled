//! One held peer process per viewed pane and stream.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read};
use std::os::unix::process::CommandExt as _;
use std::process::Stdio;
use std::sync::{Arc, Mutex};

use tokio::sync::{mpsc, watch};

use super::AppState;
use super::agents::{PeerOperation, PeerReply};

type Key = (String, String, String);
pub(super) type Line = Result<PeerReply, String>;
type Sender = watch::Sender<Option<Line>>;

/// Retain process ownership even if the async follower is cancelled.
struct Process {
    child: std::process::Child,
    killed: bool,
}

impl Process {
    fn kill(&mut self) {
        if !self.killed {
            let _ = crate::shell::kill_group(self.child.id());
            self.killed = true;
        }
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        self.kill();
        let _ = self.child.wait();
    }
}

#[derive(Debug, Clone, Default)]
pub(super) struct Followers {
    entries: Arc<Mutex<HashMap<Key, Sender>>>,
    launcher: Arc<Launcher>,
}

#[derive(Debug)]
struct Launcher {
    program: String,
    args: Vec<String>,
    #[cfg(test)]
    calls: Mutex<Vec<Vec<String>>>,
}

impl Default for Launcher {
    fn default() -> Self {
        Self {
            program: "ssh".into(),
            args: Vec::new(),
            #[cfg(test)]
            calls: Mutex::default(),
        }
    }
}

impl Followers {
    pub(super) fn subscribe(
        &self,
        state: &AppState,
        host: String,
        pane: String,
        operation: &PeerOperation,
    ) -> crate::error::Result<watch::Receiver<Option<Line>>> {
        let stream = match operation {
            PeerOperation::Event => "event",
            PeerOperation::Activity => "activity",
            _ => {
                return Err(crate::error::Error::Invalid(
                    "unsupported peer stream".into(),
                ));
            }
        };
        let key = (host, pane, stream.to_owned());
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(sender) = entries.get(&key)
            && !sender.is_closed()
        {
            return Ok(sender.subscribe());
        }
        let (sender, receiver) = watch::channel(None);
        entries.insert(key.clone(), sender.clone());
        drop(entries);
        let followers = self.clone();
        let state = state.clone();
        let operation = operation.clone();
        tokio::spawn(async move {
            followers.follow(&state, &key, &operation, &sender).await;
            let mut entries = followers
                .entries
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if entries
                .get(&key)
                .is_some_and(|entry| entry.same_channel(&sender))
            {
                entries.remove(&key);
            }
        });
        Ok(receiver)
    }

    async fn follow(
        &self,
        state: &AppState,
        key: &Key,
        operation: &PeerOperation,
        sender: &Sender,
    ) {
        let unavailable = || format!("host {}: agent stream unavailable", key.0);
        let selected = state.clone();
        let host = key.0.clone();
        let pane = key.1.clone();
        let operation = operation.clone();
        let argv =
            tokio::task::spawn_blocking(move || remote_argv(&selected, &host, &pane, &operation))
                .await;
        let Ok(Ok(argv)) = argv else {
            sender.send_replace(Some(Err(unavailable())));
            return;
        };
        if sender.is_closed() || state.stop.is_cancelled() {
            return;
        }
        let (program, argv) = self.launch(argv);
        let child = crate::shell::command(program)
            .args(argv)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn();
        let Ok(child) = child else {
            sender.send_replace(Some(Err(unavailable())));
            return;
        };
        let mut process = Process {
            child,
            killed: false,
        };
        let stdout = process.child.stdout.take();
        let (lines, mut receiver) = mpsc::channel(1);
        let reader = tokio::task::spawn_blocking(move || {
            if let Some(stdout) = stdout {
                read_lines(stdout, &lines);
            }
        });
        let mut published = false;
        let mut exited = tokio::time::interval(std::time::Duration::from_millis(50));
        loop {
            tokio::select! {
                () = sender.closed() => break,
                () = state.stop.cancelled() => break,
                line = receiver.recv() => {
                    let Some(line) = line else { break; };
                    let failed = line.is_err();
                    published = true;
                    sender.send_replace(Some(line));
                    if failed { break; }
                }
                _ = exited.tick() => {
                    if !matches!(process.child.try_wait(), Ok(None)) {
                        // Drain the reader before reporting an empty stream.
                        process.kill();
                        while let Some(line) = receiver.recv().await {
                            published = true;
                            let failed = line.is_err();
                            sender.send_replace(Some(line));
                            if failed { break; }
                        }
                        break;
                    }
                }
            }
        }
        process.kill();
        drop(process.child.stdin.take());
        drop(receiver);
        let _ = reader.await;
        let _ = tokio::task::spawn_blocking(move || drop(process)).await;
        if !published && !sender.is_closed() && !state.stop.is_cancelled() {
            sender.send_replace(Some(Err(unavailable())));
        }
    }

    fn launch(&self, argv: Vec<String>) -> (String, Vec<String>) {
        #[cfg(test)]
        self.launcher.calls.lock().unwrap().push(argv.clone());
        let mut command = self.launcher.args.clone();
        command.extend(argv);
        (self.launcher.program.clone(), command)
    }

    #[cfg(test)]
    pub(super) fn with_launcher(program: &str, args: Vec<String>) -> Self {
        Self {
            launcher: Arc::new(Launcher {
                program: program.to_owned(),
                args,
                calls: Mutex::default(),
            }),
            ..Self::default()
        }
    }

    #[cfg(test)]
    pub(super) fn calls(&self) -> Vec<Vec<String>> {
        self.launcher.calls.lock().unwrap().clone()
    }
}

fn remote_argv(
    state: &AppState,
    host: &str,
    pane: &str,
    operation: &PeerOperation,
) -> crate::error::Result<Vec<String>> {
    let launch = state
        .leads
        .as_ref()
        .ok_or_else(|| crate::error::Error::Invalid("peer transport unavailable".into()))?;
    let policy = crate::config::load(&launch.config_path)?;
    let fleet = crate::hosts::ActionFleet::current(
        launch.shell.as_ref(),
        &state.web,
        &policy,
        launch.home.clone(),
    )?;
    let target = fleet.find(host)?;
    Ok(fleet.remote_argv(
        target,
        &[
            "agent-detail".into(),
            pane.into(),
            "--follow".into(),
            "--input".into(),
            serde_json::to_string(operation)?,
        ],
    ))
}

fn read_lines(stdout: impl Read, sender: &mpsc::Sender<Line>) {
    let mut reader = BufReader::new(stdout);
    let limit = crate::observation::details::CODEC.wire_limit();
    loop {
        let mut bytes = Vec::new();
        let read = reader
            .by_ref()
            .take(limit as u64 + 1)
            .read_until(b'\n', &mut bytes);
        let line = match read {
            Ok(0) => break,
            Ok(_) if bytes.len() > limit => Err("agent stream line exceeds limit".into()),
            Ok(_) if bytes.last() != Some(&b'\n') => Err("invalid agent stream line".into()),
            Ok(_) => serde_json::from_slice(&bytes).map_err(|_| "invalid agent stream line".into()),
            Err(_) => Err("agent stream read failed".into()),
        };
        let failed = line.is_err();
        if sender.blocking_send(line).is_err() || failed {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::time::{Duration, Instant};

    struct Fixture {
        state: AppState,
        _publisher: crate::observation::Publisher,
        server: Option<crate::observation::ipc::Server>,
    }

    impl Fixture {
        fn new(script: String) -> Self {
            let web =
                crate::ledger::Ledger::new(std::env::temp_dir().join(crate::ledger::new_id()));
            std::fs::create_dir_all(&web.dir).unwrap();
            let config_path = web.dir.join("config.toml");
            std::fs::write(&config_path, "remote_command = ['dogsled']\n").unwrap();
            let mut state = AppState::new(
                watch::channel(super::super::PortalState::default()).1,
                7465,
                None,
                super::super::guard::Token::new(String::new()),
                web,
            );
            state = state.with_leads(super::super::LeadLaunch {
                config_path,
                home: "/home/test".into(),
                shell: Arc::new(crate::shell::Fake::new()),
            });
            state.followers = Followers::with_launcher("sh", vec!["-c".into(), script]);
            let mut publisher = crate::observation::Publisher::new().unwrap();
            publisher.replace(crate::observation::Publication {
                published_ms: 1, sources: crate::observation::Sources::default(), local: crate::observation::Domains::new(),
                views: crate::observation::Domains::from([("hosts".into(), json!({"data":[{
                    "id":"peer-id","label":"netcup","target":"netcup-dev","service":"up","reachable":true,
                    "sampled_ms":1,"probe":null,"headroom":null,"error":null,"jobs":[]
                }],"gaps":[]}))]), gaps: vec![],
            }).unwrap();
            let mut server = crate::observation::ipc::Server::new(&state.web.dir).unwrap();
            server.bind(publisher.subscribe()).unwrap();
            Self {
                state,
                _publisher: publisher,
                server: Some(server),
            }
        }

        fn subscribe(&self) -> watch::Receiver<Option<Line>> {
            self.state
                .followers
                .subscribe(
                    &self.state,
                    "netcup".into(),
                    "w1:p1".into(),
                    &PeerOperation::Event,
                )
                .unwrap()
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            self.state.stop.cancel();
            self.server.take();
            std::fs::remove_dir_all(&self.state.web.dir).unwrap();
        }
    }

    async fn next(receiver: &mut watch::Receiver<Option<Line>>) -> Line {
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if let Some(line) = receiver.borrow_and_update().clone() {
                    return line;
                }
                receiver.changed().await.unwrap();
            }
        })
        .await
        .unwrap()
    }

    async fn gone(pid: i32) {
        let deadline = Instant::now() + Duration::from_secs(3);
        while nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_ok() {
            assert!(
                Instant::now() < deadline,
                "process {pid} must be killed and reaped"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    #[tokio::test]
    async fn viewers_share_lines_and_last_drop_kills_the_process() {
        let release = std::env::temp_dir().join(crate::ledger::new_id());
        let fixture = Fixture::new(format!(
            "printf '{{\"status\":200,\"body\":{{\"pid\":%s,\"value\":1}}}}\\n' \"$$\"; while [ ! -f {} ]; do sleep 0.01; done; printf '{{\"status\":200,\"body\":{{\"pid\":%s,\"value\":2}}}}\\n' \"$$\"; cat >/dev/null",
            crate::shell::quote(&release.to_string_lossy())
        ));
        let mut first = fixture.subscribe();
        let mut second = fixture.subscribe();
        let one = next(&mut first).await.unwrap();
        let pid = i32::try_from(one.body["pid"].as_i64().unwrap()).unwrap();
        assert_eq!(one.body["value"], 1);
        assert_eq!(next(&mut second).await.unwrap().body, one.body);
        std::fs::write(&release, "release").unwrap();
        tokio::time::timeout(Duration::from_secs(3), first.changed())
            .await
            .unwrap()
            .unwrap();
        tokio::time::timeout(Duration::from_secs(3), second.changed())
            .await
            .unwrap()
            .unwrap();
        std::fs::remove_file(release).unwrap();
        assert_eq!(
            first
                .borrow_and_update()
                .as_ref()
                .unwrap()
                .as_ref()
                .unwrap()
                .body["value"],
            2
        );
        assert_eq!(
            second
                .borrow_and_update()
                .as_ref()
                .unwrap()
                .as_ref()
                .unwrap()
                .body["value"],
            2
        );
        assert_eq!(fixture.state.followers.calls().len(), 1);
        drop(first);
        assert!(nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_ok());
        drop(second);
        gone(pid).await;
    }

    #[tokio::test]
    async fn a_late_viewer_reuses_the_current_line_and_closed_viewers_restart() {
        let fixture =
            Fixture::new("printf '{\"status\":200,\"body\":%s}\\n' \"$$\"; cat >/dev/null".into());
        let mut first = fixture.subscribe();
        let initial = next(&mut first).await.unwrap();
        let pid = i32::try_from(initial.body.as_i64().unwrap()).unwrap();
        let mut late = fixture.subscribe();
        assert_eq!(next(&mut late).await.unwrap().body, initial.body);
        assert_eq!(fixture.state.followers.calls().len(), 1);
        drop(first);
        drop(late);
        gone(pid).await;
        let mut replacement = fixture.subscribe();
        let pid =
            i32::try_from(next(&mut replacement).await.unwrap().body.as_i64().unwrap()).unwrap();
        assert_eq!(fixture.state.followers.calls().len(), 2);
        drop(replacement);
        gone(pid).await;
    }

    #[tokio::test]
    async fn exit_without_output_and_invalid_lines_fail_once() {
        let limit = crate::observation::details::CODEC.wire_limit();
        for (script, expected) in [
            ("exit 0".into(), "host netcup: agent stream unavailable"),
            (
                "printf 'not json\\n'; cat >/dev/null".into(),
                "invalid agent stream line",
            ),
            (
                format!(
                    "dd if=/dev/zero bs={} count=1 2>/dev/null; cat >/dev/null",
                    limit + 1
                ),
                "agent stream line exceeds limit",
            ),
        ] {
            let fixture = Fixture::new(script);
            let mut receiver = fixture.subscribe();
            assert_eq!(next(&mut receiver).await.unwrap_err(), expected);
            tokio::time::timeout(Duration::from_secs(3), receiver.changed())
                .await
                .unwrap()
                .unwrap_err();
            assert_eq!(fixture.state.followers.calls().len(), 1);
        }
    }

    #[tokio::test]
    async fn shutdown_kills_a_held_process() {
        let fixture =
            Fixture::new("printf '{\"status\":200,\"body\":%s}\\n' \"$$\"; cat >/dev/null".into());
        let mut receiver = fixture.subscribe();
        let pid = i32::try_from(next(&mut receiver).await.unwrap().body.as_i64().unwrap()).unwrap();
        fixture.state.stop.cancel();
        gone(pid).await;
    }
}
