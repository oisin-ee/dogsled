//! `dogsled sidebar`: the docked pane. Three framed panels (Hosts, Agents & Jobs, Todos) drawn
//! with ratatui, fed by a reader thread so input never waits on a read.
//!
//! Herdr has no plugin sidebar API; the pane is an ordinary plugin pane that `board --open`
//! docks on the left. Colours come from Herdr's own `[theme]` (see [`palette`]).

pub mod input;
pub mod model;
pub mod palette;
pub mod view;

use crate::board;
use crate::config::Policy;
use crate::error::{Error, Result};
use crate::ledger::Ledger;
use crate::observation::{ipc, read as publication};
use crate::shell::{Shell, System};
use crossterm::event::{self, DisableMouseCapture, EnableMouseCapture, Event};
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use model::{Data, Effect, Model};
use nix::poll::{PollFd, PollFlags, poll};
use ratatui::{Terminal, backend::CrosstermBackend};
use std::os::fd::AsFd as _;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Retry only the publication transport; never reconstruct observations.
const RETRY: Duration = Duration::from_secs(1);
/// Input is polled in slices this long; each also drains the reader's results.
const TICK: Duration = Duration::from_millis(100);
/// A notice stays in the footer this long.
const NOTICE: Duration = Duration::from_secs(5);

#[derive(Debug)]
enum Msg {
    Publication(std::sync::Arc<crate::observation::Published>),
    Failed(String),
    Notice(String),
}

/// Select already-derived views from exactly one revision. No observation capabilities.
fn read(published: &crate::observation::Published, owner: &str) -> Result<Selection> {
    let data = publication::sidebar(published, owner)?;
    let preferences = publication::view::<std::collections::BTreeMap<String, model::Saved>>(
        published,
        "sidebar_preferences",
    );
    let findings = publication::findings(published, owner)?;
    let mut gaps = published.gaps.clone();
    for domain in ["sidebars", "sidebar_preferences", "findings"] {
        if let Some(value) = published.views.get(domain) {
            let view: crate::observation::project::View<serde_json::Value> =
                serde_json::from_value(value.clone())?;
            gaps.extend(view.gaps);
        }
    }
    let mut notices: Vec<_> = gaps
        .iter()
        .map(|gap| format!("observation gap: {}", publication::describe(gap)))
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    let saved = match preferences {
        Ok(mut values) if !gaps.iter().any(|gap| gap.domain == "sidebar_preferences") => {
            Preference::Published(values.remove(owner))
        }
        Ok(_) => Preference::Unavailable,
        Err(error) => {
            notices.push(error.to_string());
            Preference::Unavailable
        }
    };
    notices.extend(findings.failures);
    notices.extend(findings.holds);
    notices.extend(findings.landed_dirty);
    Ok(Selection {
        data,
        saved,
        observed_ms: published
            .sources
            .local
            .get("status")
            .copied()
            .unwrap_or(published.published_ms),
        notice: (!notices.is_empty()).then(|| notices.join("; ")),
    })
}

enum Preference {
    Unavailable,
    Published(Option<model::Saved>),
}

struct Selection {
    data: Data,
    saved: Preference,
    observed_ms: u64,
    notice: Option<String>,
}

/// Keep the render mailbox bounded. Waiting never blocks the IPC receiver or publisher,
/// and shutdown cancels a delivery even when the terminal has stopped draining frames.
fn send(tx: &mpsc::SyncSender<Msg>, mut message: Msg, stop: &AtomicBool) -> bool {
    while !stop.load(Ordering::Relaxed) {
        match tx.try_send(message) {
            Ok(()) => return true,
            Err(mpsc::TrySendError::Full(value)) => message = value,
            Err(mpsc::TrySendError::Disconnected(_)) => return false,
        }
        std::thread::park_timeout(TICK);
    }
    false
}

fn reader(client: &ipc::Client, tx: &mpsc::SyncSender<Msg>, stop: &AtomicBool) {
    while !stop.load(Ordering::Relaxed) {
        let result = client.subscribe().and_then(|mut subscription| {
            let mut connection = subscription.current();
            let mut last = None;
            while !stop.load(Ordering::Relaxed) {
                if let ipc::Status::Disconnected(detail) = connection.status {
                    return Err(Error::Invalid(detail));
                }
                if last != Some(connection.publication.revision) {
                    last = Some(connection.publication.revision);
                    if !send(tx, Msg::Publication(connection.publication), stop) {
                        return Ok(());
                    }
                }
                connection = subscription.changed(TICK);
            }
            Ok(())
        });
        if let Err(error) = result
            && !send(tx, Msg::Failed(error.to_string()), stop)
        {
            return;
        }
        let until = Instant::now() + RETRY;
        while Instant::now() < until && !stop.load(Ordering::Relaxed) {
            std::thread::park_timeout(TICK);
        }
    }
}

fn age(data: &mut Data, elapsed_ms: u64) {
    for card in &mut data.cards {
        card.age_ms = card.age_ms.saturating_add(elapsed_ms);
    }
    for host in &mut data.hosts {
        if let Some(silent) = &mut host.silent_ms {
            *silent = silent.saturating_add(elapsed_ms);
        }
    }
}

/// Local intent wins over publications captured before its write. Acknowledgement is
/// value-based, so reconnects and service epochs cannot erase an unobserved keypress.
#[derive(Default)]
struct Preferences {
    pending: Option<model::Saved>,
    last: Option<model::Saved>,
}

impl Preferences {
    fn apply(&mut self, model: &mut Model, published: Option<model::Saved>) {
        if let Some(pending) = &self.pending {
            if published.as_ref() == Some(pending) {
                self.pending = None;
                self.last = published;
            }
            return;
        }
        if self.last != published {
            self.last.clone_from(&published);
            let restored = Model::from_saved(published.clone());
            model.focus = restored.focus;
            model.collapsed = restored.collapsed;
            model.fold_open = restored.fold_open;
            model.hosts_compact = restored.hosts_compact;
            if let Some(id) = published.and_then(|saved| saved.selected_card) {
                model.select_card_id(&id);
            }
        }
    }

    fn changed(&mut self, saved: model::Saved) {
        self.pending = Some(saved);
    }
}

/// Run what an input asked for, off the input thread. The result comes back as a footer notice.
fn perform(
    effect: Effect,
    shell: &dyn Shell,
    ledger: &Ledger,
    policy: &Policy,
    home: &str,
    owner: &str,
    published: Option<std::sync::Arc<crate::observation::Published>>,
) -> Result<String> {
    match effect {
        Effect::None => Ok(String::new()),
        Effect::Focus(card) => {
            let pane = card
                .pane
                .ok_or_else(|| Error::Invalid("no pane to focus".into()))?;
            shell.run_ok("herdr", &["agent".into(), "focus".into(), pane])?;
            Ok(String::new())
        }
        Effect::Stop(card) => {
            let fleet = crate::hosts::ActionFleet::new(
                shell,
                ledger,
                policy,
                home.to_owned(),
                published.ok_or_else(|| Error::Invalid("observation unavailable".into()))?,
            )?;
            crate::runtime::stop(&fleet, &card.id)?;
            Ok(format!("Stopped {}", card.name))
        }
        Effect::Cancel(card) => {
            let result = crate::work::cancel(ledger, &card.id)?;
            Ok(if result["status"] == "too_late" {
                format!("Too late: {} already admitted; result preserved", card.name)
            } else {
                format!("Cancelled future admission for {}", card.name)
            })
        }
        Effect::Close(card) => {
            let pane = card
                .pane
                .ok_or_else(|| Error::Invalid("no pane to close".into()))?;
            let fleet = crate::hosts::ActionFleet::new(
                shell,
                ledger,
                policy,
                home.to_owned(),
                published.ok_or_else(|| Error::Invalid("observation unavailable".into()))?,
            )?;
            let (closed, label) = crate::runtime::close(&fleet, &pane, Some(&card.host))?;
            Ok(crate::runtime::closed_line(&closed, &label)
                .trim_end()
                .to_owned())
        }
        Effect::CloseDock => {
            let socket = crate::herdr_socket::Client::from_env()?;
            board::toggle(ledger, shell, &socket, owner)?;
            Ok(String::new())
        }
    }
}

/// Puts the terminal back on drop and on panic.
struct Screen;

impl Screen {
    fn enter() -> Result<Self> {
        enable_raw_mode()?;
        crossterm::execute!(std::io::stdout(), EnterAlternateScreen, EnableMouseCapture)?;
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            restore();
            previous(info);
        }));
        Ok(Self)
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        restore();
    }
}

fn restore() {
    let _ = disable_raw_mode();
    let _ = crossterm::execute!(std::io::stdout(), DisableMouseCapture, LeaveAlternateScreen);
}

/// Observe terminal loss without reading input or waiting for crossterm's input poll.
/// Only hangup is requested: key presses and resizes cannot trigger shutdown.
/// `tty` is the terminal device itself: Darwin's poll reports POLLNVAL for `/dev/tty`.
fn terminal_lifetime(tty: std::os::fd::BorrowedFd<'_>, stop: &AtomicBool) -> Result<()> {
    // Darwin registers its read filter only with an explicit event, including POLLHUP.
    let mut fd = PollFd::new(tty, PollFlags::POLLHUP);
    while !stop.load(Ordering::Relaxed) {
        match poll(std::slice::from_mut(&mut fd), 100_u16) {
            Ok(_) => {}
            Err(nix::errno::Errno::EINTR) => continue,
            Err(error) => return Err(std::io::Error::from(error).into()),
        }
        if fd.revents().is_some_and(|events| {
            events.intersects(PollFlags::POLLHUP | PollFlags::POLLERR | PollFlags::POLLNVAL)
        }) {
            // The input poll can spin forever on EOF/EIO. There is no terminal
            // left to restore, so do not wait for that thread or its destructors.
            std::process::exit(i32::from(crate::error::EXIT_ERROR));
        }
    }
    Ok(())
}

struct Display {
    model: Model,
    preferences: Preferences,
    published: Option<std::sync::Arc<crate::observation::Published>>,
    observation_notice: Option<String>,
    notice_until: Option<Instant>,
    aged_at: Instant,
}

impl Display {
    fn new() -> Self {
        Self {
            model: Model::default(),
            preferences: Preferences::default(),
            published: None,
            observation_notice: Some("observation unavailable: connecting to dogsled serve".into()),
            notice_until: None,
            aged_at: Instant::now(),
        }
    }

    fn receive(&mut self, message: Msg, owner: &str) {
        match message {
            Msg::Publication(value) => match read(&value, owner) {
                Ok(mut selection) => {
                    age(
                        &mut selection.data,
                        crate::ledger::now_ms().saturating_sub(selection.observed_ms),
                    );
                    self.model.update(selection.data);
                    if let Preference::Published(saved) = selection.saved {
                        self.preferences.apply(&mut self.model, saved);
                    }
                    self.observation_notice = selection.notice;
                    self.aged_at = Instant::now();
                    self.published = Some(value);
                }
                Err(error) => self.observation_notice = Some(error.to_string()),
            },
            Msg::Failed(error) => {
                self.observation_notice = Some(if self.published.is_some() {
                    format!("observation disconnected/stale: {error}")
                } else {
                    error
                });
            }
            Msg::Notice(error) if error.is_empty() => {}
            Msg::Notice(error) => {
                self.model.notice = Some(error);
                self.notice_until = Some(Instant::now() + NOTICE);
            }
        }
    }

    fn tick(&mut self) {
        let elapsed = self.aged_at.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
        age(&mut self.model.data, elapsed);
        self.aged_at = Instant::now();
        if self
            .notice_until
            .is_some_and(|until| Instant::now() >= until)
        {
            self.notice_until = None;
        }
        if self.notice_until.is_none() {
            self.model.notice.clone_from(&self.observation_notice);
        }
    }

    fn save(&mut self, ledger: &Ledger, owner: &str, before: &model::Saved) {
        let saved = self.model.saved();
        if &saved != before {
            self.preferences.changed(saved.clone());
            if let Err(error) = model::save(ledger, owner, &saved) {
                self.receive(Msg::Notice(format!("sidebar preferences: {error}")), owner);
            }
        }
    }
}

pub fn command(
    shell: System,
    ledger: &Ledger,
    policy: &Policy,
    home: &str,
    owner: Option<String>,
) -> Result<()> {
    let owner = board::owner(owner, false)?.unwrap_or_default();
    let mut display = Display::new();
    let pal = palette::Palette::load_default();
    let (tx, rx) = mpsc::sync_channel(1);
    let stop = AtomicBool::new(false);
    let tty = std::io::stdin();
    if !std::io::IsTerminal::is_terminal(&tty) {
        return Err(Error::Invalid(
            "the sidebar needs a terminal on stdin".into(),
        ));
    }
    let _screen = Screen::enter()?;
    let mut terminal = Terminal::new(CrosstermBackend::new(std::io::stdout()))?;
    let client = ipc::Client::at(&ledger.dir);
    std::thread::scope(|scope| {
        let lifetime = scope.spawn(|| terminal_lifetime(tty.as_fd(), &stop));
        scope.spawn(|| reader(&client, &tx, &stop));
        let outcome = (|| -> Result<()> {
            loop {
                let area = terminal.size()?;
                let area = ratatui::layout::Rect::new(0, 0, area.width, area.height);
                while let Ok(message) = rx.try_recv() {
                    display.receive(message, &owner);
                }
                display.tick();
                let geo = view::geometry(&display.model, area, &pal);
                view::settle(&mut display.model, &geo);
                terminal.draw(|frame| {
                    view::render(&display.model, frame.area(), frame.buffer_mut(), &pal);
                })?;
                if event::poll(TICK)? {
                    let before = display.model.saved();
                    let effect = match event::read()? {
                        Event::Key(key) => input::key(&mut display.model, key),
                        Event::Mouse(mouse) => {
                            input::mouse(&mut display.model, &geo, mouse, Instant::now())
                        }
                        _ => Effect::None,
                    };
                    if effect != Effect::None {
                        let tx = tx.clone();
                        let owner = owner.clone();
                        let published = display.published.clone();
                        let shell = &shell;
                        let stop = &stop;
                        scope.spawn(move || {
                            let message =
                                perform(effect, shell, ledger, policy, home, &owner, published)
                                    .map_or_else(|e| Msg::Notice(e.to_string()), Msg::Notice);
                            send(&tx, message, stop);
                        });
                    }
                    display.save(ledger, &owner, &before);
                }
            }
        })();
        stop.store(true, Ordering::Relaxed);
        match lifetime.join() {
            Ok(result) => outcome.and(result),
            Err(panic) => std::panic::resume_unwind(panic),
        }
    })
}

#[cfg(test)]
mod tests;
