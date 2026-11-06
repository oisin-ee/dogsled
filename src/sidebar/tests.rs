use super::*;
use crate::hosts::{Fleet, Host};
use crate::observation::{Publication, Published, Publisher};
use crate::shell::Fake;
use ratatui::{Terminal, backend::TestBackend};
use serde_json::json;
use std::sync::Arc;

const WAIT: Duration = Duration::from_secs(5);
const OWNER: &str = "owner";

struct Fixture {
    ledger: Ledger,
    shell: Fake,
    policy: Policy,
    publisher: Publisher,
    server: Option<ipc::Server>,
}

impl Fixture {
    fn new() -> Self {
        let ledger = Ledger::new(std::env::temp_dir().join(crate::ledger::new_id()));
        let snapshot = serde_json::from_value(json!({"sampled_ms":10_000,"probe":{
            "host":"local","label":"local","load1":0.0,"cores":8,"busy":0.1,
            "memory":{"used_bytes":1,"total_bytes":32_000_000_000u64},"disk_free_percent":99.0,
            "agents":{"total":1,"by_status":{"idle":1}},"reservations":[],"panes":[],
            "agent_panes":[{"pane":"worker","agent":"pi","state":"idle","terminal_id":"term_worker"}]
        }})).unwrap();
        crate::host_store::put_snapshot(&ledger, &snapshot).unwrap();
        crate::items::record(
            &ledger,
            crate::items::Item {
                id: "child".into(),
                name: Some("child".into()),
                kind: "agent".into(),
                host: "local".into(),
                pane: "worker".into(),
                owner_pane: OWNER.into(),
                created_ms: 1,
                ..Default::default()
            },
        )
        .unwrap();
        Self {
            ledger,
            shell: Fake::new(),
            policy: Policy::default(),
            publisher: Publisher::new().unwrap(),
            server: None,
        }
    }

    fn capture(&self) -> Arc<Published> {
        let fleet = Fleet::producer(
            &self.shell,
            &self.ledger,
            &self.policy,
            "/".into(),
            vec![
                Host::local(),
                Host {
                    id: "peer".into(),
                    label: "remote-label".into(),
                    target: Some("peer-ssh".into()),
                },
            ],
        );
        crate::observation::captured_actions_with_terminals(
            &fleet,
            10_000,
            Some(crate::probe::Terminals {
                agents: Vec::new(),
                panes: Vec::new(),
            }),
        )
        .unwrap()
        .published
    }

    fn publish(&mut self, captured: &Published) -> Arc<Published> {
        self.publisher
            .replace(Publication {
                published_ms: captured.published_ms,
                sources: captured.sources.clone(),
                local: captured.local.clone(),
                views: captured.views.clone(),
                gaps: captured.gaps.clone(),
            })
            .unwrap()
    }

    fn serve(&mut self, captured: &Published) -> Arc<Published> {
        let published = self.publish(captured);
        let mut server = ipc::Server::new(&self.ledger.dir).unwrap();
        server.bind(self.publisher.subscribe()).unwrap();
        self.server = Some(server);
        published
    }

    fn isolate(&self) -> Vec<std::fs::File> {
        std::fs::remove_file(self.ledger.dir.join("state.db")).unwrap();
        std::fs::create_dir(self.ledger.dir.join("state.db")).unwrap();
        ["state-db", "service", "host", "branch-checks"]
            .into_iter()
            .map(|name| {
                let file = std::fs::File::options()
                    .create(true)
                    .truncate(false)
                    .read(true)
                    .write(true)
                    .open(self.ledger.dir.join(format!("{name}.lock")))
                    .unwrap();
                file.try_lock().unwrap();
                file
            })
            .collect()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.take();
        std::fs::remove_dir_all(&self.ledger.dir).unwrap();
    }
}

struct Reader {
    rx: mpsc::Receiver<Msg>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Reader {
    fn start(ledger: &Ledger) -> Self {
        let client = ipc::Client::at(&ledger.dir);
        let (tx, rx) = mpsc::sync_channel(1);
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = Arc::clone(&stop);
        let thread = std::thread::spawn(move || reader(&client, &tx, &stopping));
        Self {
            rx,
            stop,
            thread: Some(thread),
        }
    }

    fn revision(&self, expected: u64) -> Msg {
        let deadline = Instant::now() + WAIT;
        loop {
            let message = self
                .rx
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .unwrap();
            if let Msg::Publication(value) = &message {
                assert!(value.revision <= expected, "unexpected revision: {value:?}");
                if value.revision == expected {
                    return message;
                }
            } else {
                panic!("expected publication, got {message:?}");
            }
        }
    }
}

impl Drop for Reader {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
    }
}

fn render(display: &mut Display) -> String {
    display.tick();
    let mut terminal = Terminal::new(TestBackend::new(180, 40)).unwrap();
    terminal
        .draw(|frame| {
            view::render(
                &display.model,
                frame.area(),
                frame.buffer_mut(),
                &palette::Palette::catppuccin(),
            );
        })
        .unwrap();
    let buffer = terminal.backend().buffer();
    buffer
        .content
        .chunks(180)
        .map(|line| {
            line.iter()
                .map(ratatui::buffer::Cell::symbol)
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn reader_isolation_renders_publication_without_database_or_observation_capabilities() {
    let mut fixture = Fixture::new();
    let captured = fixture.capture();
    let published = fixture.serve(&captured);
    let calls = fixture.shell.calls.lock().unwrap().len();
    let _locks = fixture.isolate();
    let reader = Reader::start(&fixture.ledger);
    let mut display = Display::new();
    display.receive(reader.revision(published.revision), OWNER);
    let text = render(&mut display);
    assert!(text.contains("child"), "{text}");
    assert!(text.contains("remote-label"), "{text}");
    assert_eq!(display.model.data.cards.len(), 1);
    assert_eq!(
        fixture.shell.calls.lock().unwrap().len(),
        calls,
        "reader has no shell capability"
    );
    assert!(fixture.ledger.dir.join("state.db").is_dir());
}

#[test]
fn scaling_sidebars_add_no_collection_settlement_git_or_herdr_calls() {
    let mut fixture = Fixture::new();
    let captured = fixture.capture();
    let first = fixture.serve(&captured);
    let calls = fixture.shell.calls.lock().unwrap().len();
    let _locks = fixture.isolate();
    let readers: Vec<_> = (0..8).map(|_| Reader::start(&fixture.ledger)).collect();
    for reader in &readers {
        let mut display = Display::new();
        display.receive(reader.revision(first.revision), OWNER);
        assert!(render(&mut display).contains("child"));
    }
    let second = fixture.publish(&captured);
    for reader in &readers {
        let mut display = Display::new();
        display.receive(reader.revision(second.revision), OWNER);
        assert!(render(&mut display).contains("child"));
        assert!(
            reader.rx.try_recv().is_err(),
            "unchanged publication is not polled again"
        );
    }
    assert_eq!(fixture.shell.calls.lock().unwrap().len(), calls);
    assert!(fixture.ledger.dir.join("state.db").is_dir());
}

#[test]
fn cold_unavailable_disconnect_and_restart_keep_the_last_frame_marked_stale() {
    let mut fixture = Fixture::new();
    let reader = Reader::start(&fixture.ledger);
    let mut display = Display::new();
    display.receive(reader.rx.recv_timeout(WAIT).unwrap(), OWNER);
    assert!(render(&mut display).contains("observation unavailable"));
    assert_eq!(display.model.data.cards, []);
    assert!(fixture.shell.calls.lock().unwrap().is_empty());
    let captured = fixture.capture();
    let first = fixture.serve(&captured);
    display.receive(reader.revision(first.revision), OWNER);
    assert!(render(&mut display).contains("child"));
    fixture.server.take();
    display.receive(reader.rx.recv_timeout(WAIT).unwrap(), OWNER);
    let text = render(&mut display);
    assert!(text.contains("observation disconnected/stale"), "{text}");
    assert!(text.contains("child"));
    fixture.publisher = Publisher::new().unwrap();
    let next = fixture.serve(&captured);
    assert_ne!(first.epoch, next.epoch);
    display.receive(reader.revision(next.revision), OWNER);
    let text = render(&mut display);
    assert!(text.contains("child"));
    assert!(!text.contains("disconnected/stale"), "{text}");
}

#[test]
fn a_publication_gap_is_shown_in_the_footer_and_never_written_to_the_terminal() {
    // The sidebar draws in raw mode on the terminal that is also its stderr. Run the read in
    // a child test process so its real stderr can be inspected.
    const CHILD: &str = "DOGSLED_TEST_SIDEBAR_GAP_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "sidebar::tests::a_publication_gap_is_shown_in_the_footer_and_never_written_to_the_terminal",
                "--test-threads=1",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "{}\n{stderr}",
            String::from_utf8_lossy(&output.stdout)
        );
        assert!(!stderr.contains("observation gap"), "{stderr}");
        return;
    }
    let fixture = Fixture::new();
    let mut published = (*fixture.capture()).clone();
    let gap = crate::observation::Gap {
        domain: "holds/job-validity/r-1".into(),
        availability: crate::observation::Availability::Failed,
        detail: "job-validity/r-1: no recorded owner pane".into(),
    };
    published.gaps.push(gap.clone());
    for domain in ["sidebars", "sidebar_preferences", "findings"] {
        published.views.get_mut(domain).unwrap()["gaps"] =
            serde_json::to_value(vec![gap.clone()]).unwrap();
    }
    let mut display = Display::new();
    display.receive(Msg::Publication(Arc::new(published)), OWNER);
    display.tick();
    let notice = display.model.notice.clone().unwrap_or_default();
    assert_eq!(
        notice.matches("no recorded owner pane").count(),
        1,
        "{notice}"
    );
}

#[test]
fn a_todo_publication_gap_becomes_a_notice_not_a_failed_read() {
    let fixture = Fixture::new();
    let mut published = (*fixture.capture()).clone();
    // A retained partial view is valid data, but it never hides the failed todo domain.
    let gap = crate::observation::Gap {
        domain: "todos".into(),
        availability: crate::observation::Availability::Failed,
        detail: "bad cache".into(),
    };
    published.views.get_mut("sidebars").unwrap()["gaps"] = serde_json::to_value(vec![gap]).unwrap();
    let mut display = Display::new();
    display.receive(Msg::Publication(Arc::new(published)), OWNER);
    let text = render(&mut display);
    assert!(text.contains("child"), "{text}");
    assert!(text.contains("todos: Failed: bad cache"), "{text}");
    assert!(display.model.data.todos.is_none());
    let mut empty = Display::new();
    empty.receive(Msg::Publication(fixture.capture()), "unknown-owner");
    assert_eq!(empty.model.data.cards, []);
    assert!(empty.model.data.todos.is_none());
    assert!(!render(&mut empty).contains("bad cache"));
}

#[test]
fn remote_close_uses_the_cards_published_host_not_local() {
    let fixture = Fixture::new();
    let published = fixture.capture();
    let mut card = model::Card::from_row(&model::tests::row(
        "remote",
        crate::board::Group::ToLand,
        "done",
        1,
    ));
    card.host = "remote-label".into();
    let pane = card.pane.clone().unwrap();
    let peer = Host {
        id: "peer".into(),
        label: card.host.clone(),
        target: Some("peer-ssh".into()),
    };
    let fleet = Fleet::cold(
        &fixture.shell,
        &fixture.ledger,
        &fixture.policy,
        "/".into(),
        vec![Host::local()],
    );
    let command = format!(
        "ssh {}",
        fleet
            .remote_argv(&peer, &["close".into(), pane.clone()])
            .join(" ")
    );
    fixture.shell.on(
        &command,
        &serde_json::to_string(&crate::close::Closed {
            pane: pane.clone(),
            worktree: None,
            pane_closed: true,
            retained_branch: None,
            recovery: None,
        })
        .unwrap(),
    );
    let calls = fixture.shell.calls.lock().unwrap().len();
    let result = perform(
        Effect::Close(card),
        &fixture.shell,
        &fixture.ledger,
        &fixture.policy,
        "/",
        OWNER,
        Some(published),
    )
    .unwrap();
    assert_eq!(result, format!("closed pane {pane} on remote-label"));
    assert_eq!(&fixture.shell.calls.lock().unwrap()[calls..], &[command]);
}

#[test]
fn preference_writes_wake_serve_and_stale_frames_do_not_erase_local_intent() {
    let fixture = Fixture::new();
    let before = fixture.capture();
    let socket =
        std::os::unix::net::UnixDatagram::bind(fixture.ledger.dir.join("observation-wake.sock"))
            .unwrap();
    socket.set_read_timeout(Some(WAIT)).unwrap();
    let mut display = Display::new();
    display.receive(Msg::Publication(Arc::clone(&before)), OWNER);
    let saved = display.model.saved();
    display.model.focus = model::Panel::Todos;
    display.model.fold_open = true;
    display.model.toggle_collapse(model::Panel::Agents);
    display.save(&fixture.ledger, OWNER, &saved);
    let intent = display.model.saved();
    let mut bytes = [0; 16];
    let count = socket.recv(&mut bytes).unwrap();
    assert_eq!(&bytes[..count], b"ledger");
    display.receive(Msg::Publication(before), OWNER);
    assert_eq!(display.model.saved(), intent);
    assert_eq!(display.preferences.pending, Some(intent.clone()));
    display.receive(Msg::Publication(fixture.capture()), OWNER);
    assert_eq!(display.model.saved(), intent);
    assert!(display.preferences.pending.is_none());
    let mut restarted = Display::new();
    restarted.receive(Msg::Publication(fixture.capture()), OWNER);
    assert_eq!(restarted.model.saved(), intent);
}

#[test]
fn a_preferences_gap_keeps_local_intent_and_the_other_panels() {
    let fixture = Fixture::new();
    let published = fixture.capture();
    let mut display = Display::new();
    display.receive(Msg::Publication(Arc::clone(&published)), OWNER);
    let before = display.model.saved();
    display.model.focus = model::Panel::Todos;
    display.save(&fixture.ledger, OWNER, &before);
    let intended = display.model.saved();
    let mut failed = (*published).clone();
    let gap = crate::observation::Gap {
        domain: "sidebar_preferences".into(),
        availability: crate::observation::Availability::Failed,
        detail: "preferences unavailable".into(),
    };
    failed.gaps.push(gap.clone());
    failed.views.get_mut("sidebar_preferences").unwrap()["gaps"] =
        serde_json::to_value(vec![gap]).unwrap();
    display.receive(Msg::Publication(Arc::new(failed)), OWNER);
    assert_eq!(display.model.saved(), intended);
    assert_eq!(display.preferences.pending.as_ref(), Some(&intended));
    let text = render(&mut display);
    assert!(text.contains("child"), "{text}");
    assert!(
        text.contains("sidebar_preferences: Failed: preferences unavailable"),
        "{text}"
    );
}

#[test]
fn elapsed_time_changes_age_not_published_classifications() {
    let fixture = Fixture::new();
    let mut data = read(&fixture.capture(), OWNER).unwrap().data;
    let original = data.clone();
    age(&mut data, 60_000);
    assert_eq!(data.cards[0].age_ms, original.cards[0].age_ms + 60_000);
    for (row, original) in data.hosts.iter().zip(&original.hosts) {
        assert_eq!(
            (row.memory, row.disk, row.state),
            (original.memory, original.disk, original.state)
        );
    }
}
