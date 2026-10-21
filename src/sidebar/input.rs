//! Keys and mouse to model changes. Each returns the [`Effect`] the caller must run, if any.

use super::model::{Effect, MenuItem, Model, Panel};
use super::view::{Geometry, Hit};
use crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use std::time::{Duration, Instant};

/// Two clicks on one spot within this time are a double click.
const DOUBLE_CLICK: Duration = Duration::from_millis(400);

pub fn key(model: &mut Model, event: KeyEvent) -> Effect {
    if event.kind == KeyEventKind::Release {
        return Effect::None;
    }
    model.notice = None;
    if event.modifiers.contains(KeyModifiers::CONTROL) {
        return if event.code == KeyCode::Char('c') {
            Effect::CloseDock
        } else {
            Effect::None
        };
    }
    if model.menu.is_some() {
        return menu_key(model, event.code);
    }
    match event.code {
        KeyCode::Tab | KeyCode::BackTab => model.focus_next(),
        KeyCode::Up | KeyCode::Char('k') => model.move_by(-1),
        KeyCode::Down | KeyCode::Char('j') => model.move_by(1),
        KeyCode::Home | KeyCode::Char('g') => model.move_to(false),
        KeyCode::End | KeyCode::Char('G') => model.move_to(true),
        KeyCode::Char(' ') => model.toggle_collapse(model.focus),
        KeyCode::Enter => return activate(model),
        KeyCode::Right | KeyCode::Char('l') if model.focus == Panel::Todos => {
            model.open_todo(false);
        }
        KeyCode::Left | KeyCode::Char('h') if model.focus == Panel::Todos => model.close_todo(),
        KeyCode::Char('m') if model.focus == Panel::Agents => model.open_menu(),
        _ => {}
    }
    Effect::None
}

/// `⏎`: focus the selected card's pane, or open the selected todo row.
fn activate(model: &mut Model) -> Effect {
    match model.focus {
        Panel::Hosts => Effect::None,
        Panel::Agents => model.effect_for(MenuItem::Focus),
        Panel::Todos => {
            model.open_todo(true);
            Effect::None
        }
    }
}

fn menu_key(model: &mut Model, code: KeyCode) -> Effect {
    let Some(menu) = model.menu.as_mut() else {
        return Effect::None;
    };
    if let Some(item) = menu.confirming {
        return match code {
            KeyCode::Char('y' | 'Y') => {
                model.menu = None;
                model.effect_for(item)
            }
            KeyCode::Char('n' | 'N') | KeyCode::Esc => {
                menu.confirming = None;
                Effect::None
            }
            _ => Effect::None,
        };
    }
    match code {
        KeyCode::Up | KeyCode::Char('k') => menu.selected = menu.selected.saturating_sub(1),
        KeyCode::Down | KeyCode::Char('j') => {
            menu.selected = (menu.selected + 1).min(menu.items.len() - 1);
        }
        KeyCode::Esc | KeyCode::Char('m') => model.menu = None,
        KeyCode::Enter => {
            let item = menu.items[menu.selected];
            if item.destructive() {
                menu.confirming = Some(item);
            } else {
                model.menu = None;
                return model.effect_for(item);
            }
        }
        _ => {}
    }
    Effect::None
}

pub fn mouse(model: &mut Model, geo: &Geometry, event: MouseEvent, now: Instant) -> Effect {
    let (x, y) = (event.column, event.row);
    match event.kind {
        MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
            let delta = if event.kind == MouseEventKind::ScrollUp {
                -1
            } else {
                1
            };
            if let Some(Hit::Card(_) | Hit::Empty(Panel::Agents) | Hit::Frame(Panel::Agents)) =
                geo.hit(x, y)
            {
                model.scroll_by(Panel::Agents, delta);
            } else if let Some(Hit::Todo(_) | Hit::Empty(Panel::Todos) | Hit::Frame(Panel::Todos)) =
                geo.hit(x, y)
            {
                model.scroll_by(Panel::Todos, delta);
            }
            Effect::None
        }
        MouseEventKind::Down(MouseButton::Left) => {
            let double = model.clicked.is_some_and(|(cx, cy, at)| {
                (cx, cy) == (x, y) && now.duration_since(at) <= DOUBLE_CLICK
            });
            model.clicked = if double { None } else { Some((x, y, now)) };
            model.notice = None;
            click(model, geo.hit(x, y), double)
        }
        _ => Effect::None,
    }
}

fn click(model: &mut Model, hit: Option<Hit>, double: bool) -> Effect {
    match hit {
        Some(Hit::Close) => Effect::CloseDock,
        Some(Hit::Card(index)) => {
            model.menu = None;
            model.focus = Panel::Agents;
            model.card = index;
            model.follow = true;
            if double {
                model.effect_for(MenuItem::Focus)
            } else {
                Effect::None
            }
        }
        Some(Hit::Todo(index)) => {
            model.focus = Panel::Todos;
            model.todo = index;
            model.follow = true;
            if double {
                model.open_todo(true);
            }
            Effect::None
        }
        Some(Hit::Frame(panel) | Hit::Empty(panel)) => {
            if model.focus != panel {
                model.menu = None;
            }
            model.focus = panel;
            Effect::None
        }
        None => Effect::None,
    }
}

#[cfg(test)]
mod tests {
    use super::super::model::tests::{item, row, todos};
    use super::super::model::{Data, Menu};
    use super::super::palette::Palette;
    use super::super::view::geometry;
    use super::*;
    use crate::board::{Board, Group, TodoStatus};
    use crossterm::event::KeyEventState;
    use ratatui::layout::Rect;

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    fn model() -> Model {
        let board = Board {
            hosts: vec![],
            rows: vec![
                row("a", Group::ToLand, "idle", 3),
                row("b", Group::Running, "working", 1),
                row("j", Group::Running, "running", 2),
            ],
        };
        let mut model = Model::default();
        model.update(Data::from_board(
            &board,
            Some(todos([
                vec![item(TodoStatus::InProgress, "now", None)],
                vec![item(TodoStatus::Pending, "next", None)],
                vec![item(TodoStatus::Completed, "done", None)],
                vec![],
            ])),
            vec![],
        ));
        model
    }

    fn click_at(x: u16, y: u16) -> MouseEvent {
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: x,
            row: y,
            modifiers: KeyModifiers::NONE,
        }
    }

    fn geo(model: &Model) -> Geometry {
        geometry(model, Rect::new(0, 0, 36, 37), &Palette::catppuccin())
    }

    #[test]
    fn keys_move_switch_panel_and_collapse() {
        let mut model = model();
        key(&mut model, press(KeyCode::Down));
        key(&mut model, press(KeyCode::Char('j')));
        assert_eq!(model.card, 2);
        key(&mut model, press(KeyCode::Char('k')));
        assert_eq!(model.card, 1);
        key(&mut model, press(KeyCode::Tab));
        assert_eq!(model.focus, Panel::Todos);
        key(&mut model, press(KeyCode::Char(' ')));
        assert!(model.is_collapsed(Panel::Todos));
        // Hosts comes next, then Agents & Jobs.
        key(&mut model, press(KeyCode::Tab));
        assert_eq!(model.focus, Panel::Hosts);
        key(&mut model, press(KeyCode::Char(' ')));
        assert!(model.hosts_compact);
        key(&mut model, press(KeyCode::Tab));
        assert_eq!(model.focus, Panel::Agents);
    }

    #[test]
    fn enter_focuses_a_card_and_opens_a_todo_row() {
        let mut model = model();
        let Effect::Focus(card) = key(&mut model, press(KeyCode::Enter)) else {
            panic!("expected focus");
        };
        assert_eq!(card.id, "a");
        key(&mut model, press(KeyCode::Tab));
        assert_eq!(key(&mut model, press(KeyCode::Enter)), Effect::None);
        assert_eq!(model.expanded.len(), 1);
        key(&mut model, press(KeyCode::Left));
        assert!(model.expanded.is_empty());
        key(&mut model, press(KeyCode::Right));
        assert_eq!(model.expanded.len(), 1);
    }

    #[test]
    fn destructive_menu_items_confirm_inline_and_n_backs_out() {
        let mut model = model();
        model.card = 2; // the job
        key(&mut model, press(KeyCode::Char('m')));
        assert_eq!(
            model.menu.as_ref().map(|m| m.items.clone()),
            Some(vec![MenuItem::Focus, MenuItem::StopJob])
        );
        key(&mut model, press(KeyCode::Down));
        assert_eq!(key(&mut model, press(KeyCode::Enter)), Effect::None);
        assert_eq!(
            model.menu.as_ref().and_then(|m| m.confirming),
            Some(MenuItem::StopJob)
        );
        assert_eq!(key(&mut model, press(KeyCode::Char('n'))), Effect::None);
        assert!(model.menu.as_ref().is_some_and(|m| m.confirming.is_none()));
        key(&mut model, press(KeyCode::Enter));
        let Effect::Stop(card) = key(&mut model, press(KeyCode::Char('y'))) else {
            panic!("expected stop");
        };
        assert_eq!(card.id, "j");
        assert!(model.menu.is_none());
    }

    #[test]
    fn focus_in_the_menu_runs_at_once_and_escape_closes_it() {
        let mut model = model();
        key(&mut model, press(KeyCode::Char('m')));
        assert!(matches!(
            key(&mut model, press(KeyCode::Enter)),
            Effect::Focus(_)
        ));
        key(&mut model, press(KeyCode::Char('m')));
        key(&mut model, press(KeyCode::Esc));
        assert_eq!(model.menu, None::<Menu>);
    }

    #[test]
    fn ctrl_c_closes_the_dock_and_releases_are_ignored() {
        let mut model = model();
        let mut event = press(KeyCode::Char('c'));
        event.modifiers = KeyModifiers::CONTROL;
        assert_eq!(key(&mut model, event), Effect::CloseDock);
        let mut release = press(KeyCode::Down);
        release.kind = KeyEventKind::Release;
        key(&mut model, release);
        assert_eq!(model.card, 0);
    }

    #[test]
    fn click_selects_double_click_focuses_and_the_chevron_closes() {
        let mut model = model();
        let geo = geo(&model);
        let second = geo.agents.items[1].start;
        let y = geo.agents.body.y + u16::try_from(second).unwrap();
        let t0 = Instant::now();
        assert_eq!(mouse(&mut model, &geo, click_at(5, y), t0), Effect::None);
        assert_eq!(model.card, 1);
        let Effect::Focus(card) = mouse(
            &mut model,
            &geo,
            click_at(5, y),
            t0 + Duration::from_millis(100),
        ) else {
            panic!("expected focus");
        };
        assert_eq!(card.id, "b");
        // A slow second click is a new single click.
        assert_eq!(
            mouse(
                &mut model,
                &geo,
                click_at(5, y),
                t0 + Duration::from_secs(5)
            ),
            Effect::None
        );
        assert_eq!(
            mouse(&mut model, &geo, click_at(34, geo.footer.y), t0),
            Effect::CloseDock
        );
        assert_eq!(
            mouse(&mut model, &geo, click_at(0, geo.footer.y), t0),
            Effect::None
        );
    }

    #[test]
    fn clicking_a_todo_row_focuses_its_panel_and_double_click_expands() {
        let mut model = model();
        let geo = geo(&model);
        let row = geo.todos.items[0].start;
        let y = geo.todos.body.y + u16::try_from(row).unwrap();
        let t0 = Instant::now();
        mouse(&mut model, &geo, click_at(5, y), t0);
        assert_eq!((model.focus, model.todo), (Panel::Todos, 0));
        mouse(
            &mut model,
            &geo,
            click_at(5, y),
            t0 + Duration::from_millis(50),
        );
        assert_eq!(model.expanded.len(), 1);
    }

    #[test]
    fn the_wheel_scrolls_the_panel_under_the_cursor() {
        let mut model = model();
        let geo = geo(&model);
        let wheel = |kind, y| MouseEvent {
            kind,
            column: 5,
            row: y,
            modifiers: KeyModifiers::NONE,
        };
        mouse(
            &mut model,
            &geo,
            wheel(MouseEventKind::ScrollDown, geo.todos.body.y),
            Instant::now(),
        );
        assert_eq!(model.scroll, [0, 1]);
        mouse(
            &mut model,
            &geo,
            wheel(MouseEventKind::ScrollDown, geo.agents.body.y),
            Instant::now(),
        );
        assert_eq!(model.scroll, [1, 1]);
        mouse(
            &mut model,
            &geo,
            wheel(MouseEventKind::ScrollUp, geo.agents.body.y),
            Instant::now(),
        );
        assert_eq!(model.scroll, [0, 1]);
    }
}
