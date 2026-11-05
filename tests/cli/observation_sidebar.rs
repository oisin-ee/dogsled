use super::*;

/// Native `script` supplies a PTY to the real sidebar command. The process guard owns
/// the recorder and its terminal; no alternate sidebar or observation implementation runs.
struct Sidebar {
    process: Owned,
    output: mpsc::Receiver<Vec<u8>>,
    reader: Option<std::thread::JoinHandle<()>>,
    text: String,
}

impl Sidebar {
    fn start(rig: &Rig) -> Self {
        let mut command = isolated("script");
        let run = "stty rows 40 cols 180; exec \"$DOGSLED_BIN\" sidebar --owner w1:p1";
        if cfg!(target_os = "macos") {
            command.args(["-q", "/dev/null", "sh", "-c", run]);
        } else {
            command.args(["-q", "-c", run, "/dev/null"]);
        }
        rig.rig_env(&mut command);
        command
            .env("DOGSLED_BIN", env!("CARGO_BIN_EXE_dogsled"))
            .env("TERM", "xterm-256color")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut process = Owned::spawn(&mut command);
        let mut stdout = process.child_mut().stdout.take().unwrap();
        let (sender, output) = mpsc::channel();
        let reader = std::thread::spawn(move || {
            let mut bytes = [0; 4096];
            while let Ok(count) = stdout.read(&mut bytes) {
                if count == 0 || sender.send(bytes[..count].to_vec()).is_err() {
                    break;
                }
            }
        });
        Self {
            process,
            output,
            reader: Some(reader),
            text: String::new(),
        }
    }

    fn until(&mut self, expected: &str) {
        let deadline = Instant::now() + PROCESS_DEADLINE;
        loop {
            if self.text.contains(expected) {
                return;
            }
            let bytes = self
                .output
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .unwrap_or_else(|error| {
                    panic!(
                        "sidebar did not render {expected:?}: {error}; output: {}",
                        self.text
                    )
                });
            self.text.push_str(&String::from_utf8_lossy(&bytes));
        }
    }
}

impl Drop for Sidebar {
    fn drop(&mut self) {
        self.process.terminate();
        if let Some(reader) = self.reader.take() {
            reader.join().unwrap();
        }
    }
}

fn capture(rig: &Rig) -> Value {
    let mut command = rig.cmd(&["state", "project"]);
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().unwrap();
    serde_json::to_writer(child.stdin.take().unwrap(), &rig.read("machines.json")).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "{output:?}");
    let frame: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(frame["gaps"].as_array().unwrap().is_empty(), "{frame}");
    assert!(
        frame["views"]["sidebars"]["data"]["w1:p1"].is_object(),
        "{frame}"
    );
    frame
}

fn todo(rig: &Rig, text: &str) {
    rig.put_state(&["state", "todo", "w1:p1", "--lane", "cached", "--put"], &json!({
        "session":"owner-session","revision":1,"pane":"w1:p1","current":text,
        "sections":[
            {"status":"in_progress","count":1,"items":[{"status":"in_progress","text":text,"review":false,"children":[]}]},
            {"status":"pending","count":0,"items":[]},
            {"status":"completed","count":0,"items":[]},
            {"status":"abandoned","count":0,"items":[]}
        ]
    }));
}

fn isolate(rig: &Rig) -> Vec<fs::File> {
    rig.service.lock().unwrap().take();
    for program in ["herdr", "git", "ssh"] {
        rig.script(
            program,
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$RIG/sidebar-access.log\"\nexit 97\n",
        );
    }
    fs::remove_file(rig.dir.join("state/state.db")).unwrap();
    fs::create_dir(rig.dir.join("state/state.db")).unwrap();
    ["state-db", "service", "host", "branch-checks"]
        .into_iter()
        .map(|name| {
            let file = fs::File::options()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .open(rig.dir.join(format!("state/{name}.lock")))
                .unwrap();
            file.try_lock().unwrap();
            file
        })
        .collect()
}

#[test]
fn sidebar_readers_render_supplied_state_project_revisions_without_observation() {
    let rig = Rig::short("sidebar-publication");
    rig.snapshot("state", now(), 0.0, 8);
    todo(&rig, "AAAAAAA");
    rig.put_state(
        &["state", "sidebar", "w1:p1", "--put"],
        &json!({
            "focus":"todos","collapsed":["agents"],"fold_open":true,"selected_card":null
        }),
    );
    let first = capture(&rig);
    todo(&rig, "BBBBBBB");
    let mut second = capture(&rig);
    second["epoch"] = first["epoch"].clone();
    second["revision"] = json!(2);
    let supplied = Supplied::start(&rig, first);
    let _locks = isolate(&rig);
    let mut sidebars: Vec<_> = (0..4).map(|_| Sidebar::start(&rig)).collect();
    for sidebar in &mut sidebars {
        supplied.requested("subscribe");
        sidebar.until("AAAAAAA");
    }
    supplied.replace(second);
    for sidebar in &mut sidebars {
        sidebar.until("BBBBBBB");
    }
    assert_eq!(
        rig.log("sidebar-access.log"),
        "",
        "no Git, Herdr or SSH reads as sidebar count increases"
    );
    assert!(rig.dir.join("state/state.db").is_dir());
    assert!(
        supplied.requests.try_recv().is_err(),
        "each sidebar subscribes once, never polls current"
    );
    drop(supplied);
    for sidebar in &mut sidebars {
        sidebar.until("observation disconnected/stale");
    }
    assert_eq!(rig.log("sidebar-access.log"), "");
}

#[test]
fn sidebar_cold_unavailable_does_not_read_the_database_or_discover_hosts() {
    let rig = Rig::short("sidebar-cold");
    rig.snapshot("state", now(), 0.0, 8);
    let _locks = isolate(&rig);
    let mut sidebar = Sidebar::start(&rig);
    sidebar.until("observation unavailable");
    assert_eq!(rig.log("sidebar-access.log"), "");
    assert!(rig.dir.join("state/state.db").is_dir());
}
