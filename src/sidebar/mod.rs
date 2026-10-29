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
use crate::config::{Policy, Thresholds};
use crate::error::{Error, Result};
use crate::hosts::Fleet;
use crate::ledger::{Ledger, now_ms};
use crate::shell::{Shell, System};
use crossterm::event::{self, DisableMouseCapture, EnableMouseCapture, Event};
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use model::{Data, Effect, HostRow, Model};
use ratatui::{Terminal, backend::CrosstermBackend};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// The reader rebuilds the model from cached ledger state this often.
const REFRESH: Duration = Duration::from_secs(2);
/// Input is polled in slices this long; each also drains the reader's results.
const TICK: Duration = Duration::from_millis(100);
/// A notice stays in the footer this long.
const NOTICE: Duration = Duration::from_secs(5);

enum Msg {
    Data(Data),
    Failed(String),
    Notice(String),
}

/// Build the model's data from the cached status. Never probes or reaches a peer itself:
/// `commands::status` reads the service snapshots and the peer caches in the ledger.
/// The todo cache is a separate source: when it cannot be read, Hosts and Agents & Jobs
/// still draw, and the error comes back for the footer.
fn read(fleet: &Fleet<'_>, limits: &Thresholds, owner: &str) -> Result<(Data, Option<String>)> {
    let now = now_ms();
    let status = crate::commands::status(fleet)?;
    let board = board::build(&status, Some(owner), now);
    let (todos, todo_error) = owner_todos(board::cached_todos(fleet.ledger), owner);
    let mut hosts: Vec<HostRow> = status
        .hosts
        .iter()
        .map(|report| HostRow::from_report(report, limits, now))
        .collect();
    // Local first, then config order (`Fleet::discover` already lists them so).
    if let Some(local) = hosts.iter().position(|h| {
        status
            .hosts
            .iter()
            .any(|r| r.host.is_local() && r.host.label == h.label)
    }) {
        let row = hosts.remove(local);
        hosts.insert(0, row);
    }
    Ok((Data::from_board(&board, todos, hosts), todo_error))
}

/// The owner's sections, or the cache error for the footer. Never fails the whole read.
fn owner_todos(
    cached: Result<Vec<board::TodoSections>>,
    owner: &str,
) -> (Option<board::TodoSections>, Option<String>) {
    match cached {
        Ok(todos) => (todos.into_iter().find(|todo| todo.pane == owner), None),
        Err(error) => (None, Some(format!("todos: {error}"))),
    }
}

fn reader(
    shell: &dyn Shell,
    ledger: &Ledger,
    policy: &Policy,
    home: &str,
    owner: &str,
    tx: &mpsc::Sender<Msg>,
    stop: &AtomicBool,
) {
    let limits = Thresholds::from(policy);
    let mut fleet: Option<Fleet<'_>> = None;
    while !stop.load(Ordering::Relaxed) {
        if fleet.is_none() {
            fleet = Fleet::discover(shell, ledger, policy, home.to_owned()).ok();
        }
        let messages = match &fleet {
            Some(fleet) => match read(fleet, &limits, owner) {
                Ok((data, todo_error)) => {
                    let mut messages = vec![Msg::Data(data)];
                    messages.extend(todo_error.map(Msg::Failed));
                    messages
                }
                Err(error) => vec![Msg::Failed(error.to_string())],
            },
            None => vec![Msg::Failed("herdr machine list failed".into())],
        };
        if messages
            .into_iter()
            .any(|message| tx.send(message).is_err())
        {
            return;
        }
        let until = Instant::now() + REFRESH;
        while Instant::now() < until && !stop.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

/// Run what an input asked for, off the input thread. The result comes back as a footer notice.
fn perform(
    effect: Effect,
    shell: System,
    ledger: &Ledger,
    policy: &Policy,
    home: &str,
    owner: &str,
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
            let fleet = Fleet::discover(&shell, ledger, policy, home.to_owned())?;
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
            crate::close::local(
                &shell,
                ledger,
                &pane,
                &crate::close::Caller::from_env(),
                crate::close::Guard::Clean,
            )?;
            Ok(format!("Closed {}", card.name))
        }
        Effect::CloseDock => {
            let socket = crate::herdr_socket::Client::from_env()?;
            board::toggle(ledger, &shell, &socket, owner)?;
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

pub fn command(
    shell: System,
    ledger: &Ledger,
    policy: &Policy,
    home: &str,
    owner: Option<String>,
) -> Result<()> {
    let owner = board::owner(owner, false)?.unwrap_or_default();
    let mut model = Model::from_saved(model::load(ledger, &owner));
    let saved_card = model::load(ledger, &owner).and_then(|s| s.selected_card);
    let pal = palette::Palette::load_default();
    let (tx, rx) = mpsc::channel();
    let stop = AtomicBool::new(false);
    let _screen = Screen::enter()?;
    let mut terminal = Terminal::new(CrosstermBackend::new(std::io::stdout()))?;
    let mut last_saved = model.saved();
    let mut notice_until: Option<Instant> = None;
    let mut first = true;
    std::thread::scope(|scope| {
        scope.spawn(|| reader(&shell, ledger, policy, home, &owner, &tx, &stop));
        let outcome = (|| -> Result<()> {
            loop {
                let area = terminal.size()?;
                let area = ratatui::layout::Rect::new(0, 0, area.width, area.height);
                while let Ok(message) = rx.try_recv() {
                    match message {
                        Msg::Data(data) => {
                            model.update(data);
                            if first {
                                first = false;
                                if let Some(id) = &saved_card {
                                    model.select_card_id(id);
                                }
                            }
                        }
                        Msg::Failed(error) | Msg::Notice(error) if error.is_empty() => {}
                        Msg::Failed(error) | Msg::Notice(error) => {
                            model.notice = Some(error);
                            notice_until = Some(Instant::now() + NOTICE);
                        }
                    }
                }
                if notice_until.is_some_and(|t| Instant::now() >= t) {
                    model.notice = None;
                    notice_until = None;
                }
                let geo = view::geometry(&model, area, &pal);
                view::settle(&mut model, &geo);
                terminal
                    .draw(|frame| view::render(&model, frame.area(), frame.buffer_mut(), &pal))?;
                if event::poll(TICK)? {
                    let effect = match event::read()? {
                        Event::Key(key) => input::key(&mut model, key),
                        Event::Mouse(mouse) => {
                            input::mouse(&mut model, &geo, mouse, Instant::now())
                        }
                        _ => Effect::None,
                    };
                    if effect != Effect::None {
                        let tx = tx.clone();
                        let owner = owner.clone();
                        let (shell, ledger, policy) = (shell, ledger, policy);
                        scope.spawn(move || {
                            let message = perform(effect, shell, ledger, policy, home, &owner)
                                .map_or_else(|e| Msg::Notice(e.to_string()), Msg::Notice);
                            let _ = tx.send(message);
                        });
                    }
                    let saved = model.saved();
                    if saved != last_saved {
                        model::save(ledger, &owner, &saved);
                        last_saved = saved;
                    }
                }
            }
        })();
        stop.store(true, Ordering::Relaxed);
        outcome
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_todo_cache_error_becomes_a_notice_not_a_failed_read() {
        let (todos, error) = owner_todos(Err(Error::Invalid("bad cache".into())), "w1:p1");
        assert_eq!(todos, None);
        assert_eq!(error.as_deref(), Some("todos: bad cache"));
        let (todos, error) = owner_todos(Ok(Vec::new()), "w1:p1");
        assert_eq!((todos, error), (None, None));
    }
}
