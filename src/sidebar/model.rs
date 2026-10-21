//! What the sidebar knows: cards, todo rows, and the owner's selection, fold, and scroll state.
//! Nothing here draws or blocks. The reader thread builds [`Data`]; input mutates [`Model`].

use crate::board::{Board, Group, Row, TodoSections, TodoStatus};
use crate::commands::HostState;
use crate::config::Thresholds;
use crate::hosts::HostReport;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Panel {
    Hosts,
    Agents,
    Todos,
}

impl Panel {
    /// `tab` order: Hosts, Agents & Jobs, Todos, and around.
    pub fn next(self) -> Self {
        match self {
            Self::Hosts => Self::Agents,
            Self::Agents => Self::Todos,
            Self::Todos => Self::Hosts,
        }
    }
}

/// The colour family of a card's state bar and pill.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    ToLand,
    Alert,
    Busy,
    Idle,
    Queued,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Card {
    pub id: String,
    pub kind: String,
    pub name: String,
    /// `pi`, `codex`, `job`: the first part of the board's `what`.
    pub harness: String,
    pub host: String,
    pub pane: Option<String>,
    pub group: Group,
    /// The pill text: `to land`, `stalled`, `blocked`, `working`, `idle`, `running`, `queued`, `failed`.
    pub pill: String,
    pub tone: Tone,
    pub ahead: Option<u64>,
    pub age_ms: u64,
    pub activity: String,
}

impl Card {
    pub fn from_row(row: &Row) -> Self {
        let (pill, tone) = if row.group == Group::ToLand {
            ("to land", Tone::ToLand)
        } else {
            match row.state.as_str() {
                "stalled" => ("stalled", Tone::Alert),
                "blocked" => ("blocked", Tone::Alert),
                "working" => ("working", Tone::Busy),
                "running" => ("running", Tone::Busy),
                "idle" | "done" => ("idle", Tone::Idle),
                "queued" | "starting" => ("queued", Tone::Queued),
                state if state.starts_with("exit ") => ("failed", Tone::Alert),
                _ if row.group == Group::Attention => ("blocked", Tone::Alert),
                _ => ("queued", Tone::Queued),
            }
        };
        let harness = row.what.split(" · ").next().unwrap_or(&row.what);
        Self {
            id: row.id.clone(),
            kind: row.kind.clone(),
            name: row.name.clone(),
            harness: harness.to_owned(),
            host: row.host.clone(),
            pane: row.pane.clone(),
            group: row.group,
            pill: pill.to_owned(),
            tone,
            ahead: row.ahead.filter(|n| *n > 0),
            age_ms: row.age_ms,
            activity: row
                .activity
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" "),
        }
    }

    /// `pi · netcup`, `job · local`.
    pub fn subtitle(&self) -> String {
        format!("{} · {}", self.harness, self.host)
    }
}

/// How close a host is to the limit admission enforces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Ok,
    /// Close to the limit.
    Warn,
    /// Past it: admission would refuse.
    Bad,
}

/// One host of the Hosts panel, from the same report `dogsled status` prints.
#[derive(Debug, Clone, PartialEq)]
pub struct HostRow {
    pub label: String,
    /// `11/12`; see [`crate::commands::HostBrief`].
    pub load: String,
    pub state: HostState,
    /// False until the host reported a probe; the row then shows `?` only.
    pub measured: bool,
    /// Memory in use as a fraction, and how close it is to the admission reserve.
    pub memory: Option<(f64, Level)>,
    /// Free disk percent, and how close it is to `disk_free_min_percent`.
    pub disk: Option<(f64, Level)>,
    /// Age of the last snapshot, for an unreachable host.
    pub silent_ms: Option<u64>,
}

impl HostRow {
    pub fn from_report(report: &HostReport, limits: &Thresholds, now: u64) -> Self {
        let brief = crate::commands::HostBrief::of(report);
        let memory = report.probe.as_ref().map(|p| {
            let free = p.memory.total_bytes.saturating_sub(p.memory.used_bytes) as f64;
            // The admission rule: the reserve for every held reservation plus the new agent.
            let need = limits
                .memory_reserve_bytes
                .saturating_mul(p.reservations.len() as u64 + 1) as f64;
            let level = if free < need {
                Level::Bad
            } else if free < need * 1.1 {
                Level::Warn
            } else {
                Level::Ok
            };
            (
                p.memory.used_bytes as f64 / (p.memory.total_bytes.max(1) as f64),
                level,
            )
        });
        let disk = report.probe.as_ref().map(|p| {
            let min = limits.disk_free_min_percent;
            let level = if p.disk_free_percent < min {
                Level::Bad
            } else if p.disk_free_percent < 2.0 * min {
                Level::Warn
            } else {
                Level::Ok
            };
            (p.disk_free_percent, level)
        });
        Self {
            label: brief.label,
            load: brief.load,
            state: brief.state,
            measured: brief.measured,
            memory,
            disk,
            silent_ms: (brief.state == HostState::Unreachable)
                .then(|| report.sampled_ms.map(|sampled| now.saturating_sub(sampled)))
                .flatten(),
        }
    }

    pub fn unreachable(&self) -> bool {
        self.state == HostState::Unreachable
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Data {
    pub cards: Vec<Card>,
    pub todos: Option<TodoSections>,
    /// Local first, then config order.
    pub hosts: Vec<HostRow>,
}

impl Data {
    /// Groups in board order (to land, attention, running, waiting); newest first within one.
    pub fn from_board(board: &Board, todos: Option<TodoSections>, hosts: Vec<HostRow>) -> Self {
        let mut cards: Vec<Card> = board.rows.iter().map(Card::from_row).collect();
        cards.sort_by(|a, b| a.group.cmp(&b.group).then(a.age_ms.cmp(&b.age_ms)));
        Self {
            cards,
            todos,
            hosts,
        }
    }

    /// A host that stopped answering: its last cards stay, dimmed.
    pub fn is_stale(&self, host: &str) -> bool {
        self.hosts
            .iter()
            .any(|h| h.label == host && h.unreachable())
    }

    fn count(&self, status: TodoStatus) -> usize {
        self.todos.as_ref().map_or(0, |todos| {
            todos
                .sections
                .iter()
                .find(|s| s.status == status)
                .map_or(0, |s| s.count.max(s.items.len()))
        })
    }

    pub fn done(&self) -> usize {
        self.count(TodoStatus::Completed)
    }

    pub fn dropped(&self) -> usize {
        self.count(TodoStatus::Abandoned)
    }

    pub fn total(&self) -> usize {
        self.done()
            + self.dropped()
            + self.count(TodoStatus::InProgress)
            + self.count(TodoStatus::Pending)
    }
}

/// A selectable row of the Todos panel, in drawing order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TodoRow {
    /// `(section, index)`: in progress under NOW, pending under NEXT.
    Item(usize, usize),
    /// The folded `✓ N done  − N dropped` line.
    Fold,
    /// A completed or dropped item, listed once the fold is open.
    Closed(usize, usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuItem {
    Focus,
    StopJob,
    CancelQueued,
    CloseFinished,
}

impl MenuItem {
    pub fn label(self) -> &'static str {
        match self {
            Self::Focus => "Focus",
            Self::StopJob => "Stop job",
            Self::CancelQueued => "Cancel queued",
            Self::CloseFinished => "Close finished child",
        }
    }

    pub fn destructive(self) -> bool {
        !matches!(self, Self::Focus)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Menu {
    pub items: Vec<MenuItem>,
    pub selected: usize,
    /// Set while a destructive item waits for `y/n`.
    pub confirming: Option<MenuItem>,
}

/// What the caller must do after an input; the model itself never runs a command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    None,
    Focus(Card),
    Stop(Card),
    Cancel(Card),
    Close(Card),
    CloseDock,
}

/// Only these survive a restart (`sidebar/<owner>.json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Saved {
    pub focus: Panel,
    pub collapsed: Vec<Panel>,
    pub fold_open: bool,
    #[serde(default)]
    pub selected_card: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Model {
    pub data: Data,
    pub focus: Panel,
    pub collapsed: BTreeSet<u8>,
    pub fold_open: bool,
    /// Hosts folded to one line per host. Hosts never scrolls and never hides.
    pub hosts_compact: bool,
    pub card: usize,
    pub todo: usize,
    /// Row offsets of each panel's viewport (agents, todos).
    pub scroll: [usize; 2],
    /// Keep the selection in view on the next draw. Wheel scrolling clears it.
    pub follow: bool,
    pub expanded: BTreeSet<TodoRow>,
    pub menu: Option<Menu>,
    pub notice: Option<String>,
    pub clicked: Option<(u16, u16, std::time::Instant)>,
}

impl Default for Model {
    fn default() -> Self {
        Self {
            data: Data::default(),
            focus: Panel::Agents,
            collapsed: BTreeSet::new(),
            fold_open: false,
            hosts_compact: false,
            card: 0,
            todo: 0,
            scroll: [0, 0],
            follow: true,
            expanded: BTreeSet::new(),
            menu: None,
            notice: None,
            clicked: None,
        }
    }
}

/// Agents & Jobs and Todos fold to their border line, one at a time. Hosts folds on its own.
const fn slot(panel: Panel) -> u8 {
    match panel {
        Panel::Agents => 0,
        Panel::Todos => 1,
        Panel::Hosts => 2,
    }
}

impl Model {
    pub fn from_saved(saved: Option<Saved>) -> Self {
        let mut model = Self::default();
        if let Some(saved) = saved {
            model.focus = saved.focus;
            model.hosts_compact = saved.collapsed.contains(&Panel::Hosts);
            model.collapsed = saved
                .collapsed
                .into_iter()
                .filter(|p| *p != Panel::Hosts)
                .map(slot)
                .collect();
            // One panel stays open so the other can grow into it.
            if model.collapsed.len() > 1 {
                model.collapsed.clear();
            }
            model.fold_open = saved.fold_open;
        }
        model
    }

    pub fn saved(&self) -> Saved {
        Saved {
            focus: self.focus,
            collapsed: [Panel::Hosts, Panel::Agents, Panel::Todos]
                .into_iter()
                .filter(|p| self.is_collapsed(*p))
                .collect(),
            fold_open: self.fold_open,
            selected_card: self.selected_card().map(|c| c.id.clone()),
        }
    }

    pub fn is_collapsed(&self, panel: Panel) -> bool {
        if panel == Panel::Hosts {
            return self.hosts_compact;
        }
        self.collapsed.contains(&slot(panel))
    }

    /// `space`: fold the focused panel to its border line, or open it again.
    pub fn toggle_collapse(&mut self, panel: Panel) {
        if panel == Panel::Hosts {
            self.hosts_compact = !self.hosts_compact;
            return;
        }
        let key = slot(panel);
        if !self.collapsed.remove(&key) {
            self.collapsed.clear();
            self.collapsed.insert(key);
        }
        self.follow = true;
    }

    /// Replace the data, keeping the same card selected when it still exists.
    pub fn update(&mut self, data: Data) {
        let id = self.selected_card().map(|c| c.id.clone());
        let todo = self.todo_rows().get(self.todo).copied();
        self.data = data;
        if let Some(index) = id.and_then(|id| self.data.cards.iter().position(|c| c.id == id)) {
            self.card = index;
        }
        self.card = self.card.min(self.data.cards.len().saturating_sub(1));
        let rows = self.todo_rows();
        self.expanded.retain(|row| rows.contains(row));
        self.todo = todo
            .and_then(|row| rows.iter().position(|r| *r == row))
            .unwrap_or(self.todo)
            .min(rows.len().saturating_sub(1));
        if self.menu.is_some() && self.selected_card().is_none() {
            self.menu = None;
        }
    }

    /// Restore the card selection saved with [`Saved::selected_card`].
    pub fn select_card_id(&mut self, id: &str) {
        if let Some(index) = self.data.cards.iter().position(|c| c.id == id) {
            self.card = index;
        }
    }

    pub fn selected_card(&self) -> Option<&Card> {
        self.data.cards.get(self.card)
    }

    pub fn todo_rows(&self) -> Vec<TodoRow> {
        let Some(todos) = &self.data.todos else {
            return Vec::new();
        };
        let mut rows = Vec::new();
        for (section, items) in [(0, &todos.sections[0]), (1, &todos.sections[1])] {
            rows.extend((0..items.items.len()).map(|i| TodoRow::Item(section, i)));
        }
        if self.data.done() + self.data.dropped() > 0 {
            rows.push(TodoRow::Fold);
            if self.fold_open {
                for section in [2, 3] {
                    rows.extend(
                        (0..todos.sections[section].items.len())
                            .map(|i| TodoRow::Closed(section, i)),
                    );
                }
            }
        }
        rows
    }

    pub fn focus_next(&mut self) {
        self.focus = self.focus.next();
        self.menu = None;
        self.follow = true;
    }

    pub fn move_by(&mut self, delta: isize) {
        self.follow = true;
        match self.focus {
            Panel::Hosts => {}
            Panel::Agents => {
                if self.menu.is_some() {
                    return;
                }
                self.card = step(self.card, delta, self.data.cards.len());
            }
            Panel::Todos => self.todo = step(self.todo, delta, self.todo_rows().len()),
        }
    }

    pub fn move_to(&mut self, last: bool) {
        self.follow = true;
        match self.focus {
            Panel::Hosts => {}
            Panel::Agents => {
                self.card = if last {
                    self.data.cards.len().saturating_sub(1)
                } else {
                    0
                }
            }
            Panel::Todos => {
                self.todo = if last {
                    self.todo_rows().len().saturating_sub(1)
                } else {
                    0
                }
            }
        }
    }

    /// Wheel: move the viewport, not the selection.
    pub fn scroll_by(&mut self, panel: Panel, delta: isize) {
        if panel == Panel::Hosts {
            return;
        }
        self.follow = false;
        let offset = &mut self.scroll[usize::from(slot(panel))];
        *offset = offset.saturating_add_signed(delta);
    }

    /// `⏎`/`→` on a todo row: show its full text and children in place, or open the fold.
    pub fn open_todo(&mut self, toggle: bool) {
        let Some(row) = self.todo_rows().get(self.todo).copied() else {
            return;
        };
        match row {
            TodoRow::Fold => {
                self.fold_open = if toggle { !self.fold_open } else { true };
            }
            row => {
                if !self.expanded.remove(&row) || !toggle {
                    self.expanded.insert(row);
                }
            }
        }
    }

    /// `←`: fold the row (or the lists) again.
    pub fn close_todo(&mut self) {
        let Some(row) = self.todo_rows().get(self.todo).copied() else {
            return;
        };
        match row {
            TodoRow::Fold => self.fold_open = false,
            TodoRow::Closed(..) if !self.expanded.contains(&row) => {
                self.fold_open = false;
                self.todo = self
                    .todo_rows()
                    .iter()
                    .position(|r| *r == TodoRow::Fold)
                    .unwrap_or(0);
            }
            row => {
                self.expanded.remove(&row);
            }
        }
    }

    pub fn menu_items(card: &Card) -> Vec<MenuItem> {
        let mut items = Vec::new();
        if card.pane.is_some() && card.group != Group::Waiting {
            items.push(MenuItem::Focus);
        }
        if card.kind == "job" && card.group != Group::Waiting {
            items.push(MenuItem::StopJob);
        }
        if card.group == Group::Waiting {
            items.push(MenuItem::CancelQueued);
        }
        if card.kind == "agent" && matches!(card.tone, Tone::ToLand | Tone::Idle) {
            items.push(MenuItem::CloseFinished);
        }
        items
    }

    pub fn open_menu(&mut self) {
        let Some(card) = self.selected_card() else {
            return;
        };
        let items = Self::menu_items(card);
        if items.is_empty() {
            self.notice = Some("Nothing to do with this card".into());
            return;
        }
        self.menu = Some(Menu {
            items,
            selected: 0,
            confirming: None,
        });
    }

    pub fn effect_for(&self, item: MenuItem) -> Effect {
        let Some(card) = self.selected_card().cloned() else {
            return Effect::None;
        };
        match item {
            MenuItem::Focus => Effect::Focus(card),
            MenuItem::StopJob => Effect::Stop(card),
            MenuItem::CancelQueued => Effect::Cancel(card),
            MenuItem::CloseFinished => Effect::Close(card),
        }
    }
}

fn step(index: usize, delta: isize, len: usize) -> usize {
    index
        .saturating_add_signed(delta)
        .min(len.saturating_sub(1))
}

/// `sidebar/<owner>.json`: path-safe whatever the pane id looks like.
pub fn saved_name(owner: &str) -> String {
    let safe: String = owner
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("{safe}.json")
}

pub fn load(ledger: &crate::ledger::Ledger, owner: &str) -> Option<Saved> {
    ledger
        .child("sidebar")
        .read(&saved_name(owner))
        .ok()
        .flatten()
}

pub fn save(ledger: &crate::ledger::Ledger, owner: &str, saved: &Saved) {
    let dir = ledger.child("sidebar");
    // A sidebar that cannot remember its folds still works.
    let _ = std::fs::create_dir_all(&dir.dir);
    let _ = dir.write(&saved_name(owner), saved);
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::board::{TodoItem, TodoSection};

    pub fn row(id: &str, group: Group, state: &str, age_ms: u64) -> Row {
        Row {
            group,
            kind: if state == "running" || state.starts_with("exit") {
                "job"
            } else {
                "agent"
            }
            .into(),
            id: id.into(),
            name: id.into(),
            what: "pi · astra".into(),
            host: "netcup".into(),
            pane: Some(format!("p-{id}")),
            state: state.into(),
            activity: String::new(),
            age_ms,
            quiet_ms: None,
            ahead: None,
            dirty: false,
        }
    }

    pub fn item(status: TodoStatus, text: &str, description: Option<&str>) -> TodoItem {
        TodoItem {
            status,
            text: text.into(),
            description: description.map(Into::into),
            review: false,
            unit: None,
            children: vec![],
        }
    }

    pub fn todos(sections: [Vec<TodoItem>; 4]) -> TodoSections {
        let status = [
            TodoStatus::InProgress,
            TodoStatus::Pending,
            TodoStatus::Completed,
            TodoStatus::Abandoned,
        ];
        TodoSections {
            session: "s".into(),
            branch: None,
            revision: 1,
            current: String::new(),
            pane: "w1:p1".into(),
            sections: sections
                .into_iter()
                .zip(status)
                .map(|(items, status)| TodoSection {
                    status,
                    count: items.len(),
                    items,
                })
                .collect(),
        }
    }

    fn model() -> Model {
        let board = Board {
            hosts: vec![],
            rows: vec![
                row("old-run", Group::Running, "working", 20),
                row("new-run", Group::Running, "idle", 5),
                row("land", Group::ToLand, "idle", 9),
                row("stuck", Group::Attention, "stalled", 1),
            ],
        };
        let mut model = Model::default();
        model.update(Data::from_board(
            &board,
            Some(todos([
                vec![item(TodoStatus::InProgress, "a", None)],
                vec![
                    item(TodoStatus::Pending, "b", None),
                    item(TodoStatus::Pending, "c", None),
                ],
                vec![item(TodoStatus::Completed, "d", None)],
                vec![item(TodoStatus::Abandoned, "e", None)],
            ])),
            vec![],
        ));
        model
    }

    #[test]
    fn cards_follow_board_groups_newest_first() {
        let model = model();
        let ids: Vec<_> = model.data.cards.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["land", "stuck", "new-run", "old-run"]);
        let pills: Vec<_> = model.data.cards.iter().map(|c| c.pill.as_str()).collect();
        assert_eq!(pills, ["to land", "stalled", "idle", "working"]);
    }

    #[test]
    fn pills_cover_jobs_and_queue() {
        let job = Card::from_row(&row("j", Group::Running, "running", 1));
        assert_eq!((job.pill.as_str(), job.tone), ("running", Tone::Busy));
        let failed = Card::from_row(&row("j", Group::Attention, "exit 2", 1));
        assert_eq!((failed.pill.as_str(), failed.tone), ("failed", Tone::Alert));
        let queued = Card::from_row(&row("q", Group::Waiting, "queued", 1));
        assert_eq!(
            (queued.pill.as_str(), queued.tone),
            ("queued", Tone::Queued)
        );
        assert_eq!(job.subtitle(), "pi · netcup");
    }

    #[test]
    fn selection_moves_clamps_and_survives_a_refresh() {
        let mut model = model();
        model.move_by(-1);
        assert_eq!(model.card, 0);
        model.move_by(2);
        assert_eq!(model.selected_card().unwrap().id, "new-run");
        model.move_by(10);
        assert_eq!(model.card, 3);
        // A new card arrives above; the same card stays selected.
        let mut data = model.data.clone();
        data.cards
            .insert(0, Card::from_row(&row("fresh", Group::ToLand, "idle", 1)));
        model.update(data);
        assert_eq!(model.selected_card().unwrap().id, "old-run");
        // The selected card goes away: clamp.
        let mut data = model.data.clone();
        data.cards.truncate(1);
        model.update(data);
        assert_eq!(model.card, 0);
    }

    #[test]
    fn tab_switches_panels_and_space_collapses_one_at_a_time() {
        let mut model = model();
        model.focus_next();
        assert_eq!(model.focus, Panel::Todos);
        model.toggle_collapse(Panel::Todos);
        assert!(model.is_collapsed(Panel::Todos));
        model.toggle_collapse(Panel::Agents);
        assert!(model.is_collapsed(Panel::Agents) && !model.is_collapsed(Panel::Todos));
        model.toggle_collapse(Panel::Agents);
        assert!(!model.is_collapsed(Panel::Agents));
    }

    #[test]
    fn todo_rows_fold_and_expand_in_place() {
        let mut model = model();
        assert_eq!(
            model.todo_rows(),
            [
                TodoRow::Item(0, 0),
                TodoRow::Item(1, 0),
                TodoRow::Item(1, 1),
                TodoRow::Fold
            ]
        );
        model.focus = Panel::Todos;
        model.open_todo(true);
        assert!(model.expanded.contains(&TodoRow::Item(0, 0)));
        model.open_todo(true);
        assert!(model.expanded.is_empty());
        model.move_to(true);
        model.open_todo(false);
        assert!(model.fold_open);
        assert_eq!(model.todo_rows().len(), 6);
        model.move_to(true);
        model.close_todo();
        assert!(!model.fold_open);
        assert_eq!(model.todo_rows()[model.todo], TodoRow::Fold);
    }

    #[test]
    fn wheel_scrolls_the_viewport_and_keys_follow_the_selection_again() {
        let mut model = model();
        model.scroll_by(Panel::Agents, 3);
        assert_eq!((model.scroll[0], model.follow), (3, false));
        model.scroll_by(Panel::Agents, -9);
        assert_eq!(model.scroll[0], 0);
        model.move_by(1);
        assert!(model.follow);
    }

    #[test]
    fn menu_offers_only_what_the_card_allows() {
        let job = Card::from_row(&row("j", Group::Running, "running", 1));
        assert_eq!(
            Model::menu_items(&job),
            [MenuItem::Focus, MenuItem::StopJob]
        );
        let queued = Card::from_row(&row("q", Group::Waiting, "queued", 1));
        assert_eq!(Model::menu_items(&queued), [MenuItem::CancelQueued]);
        let land = Card::from_row(&row("l", Group::ToLand, "idle", 1));
        assert_eq!(
            Model::menu_items(&land),
            [MenuItem::Focus, MenuItem::CloseFinished]
        );
        let working = Card::from_row(&row("w", Group::Running, "working", 1));
        assert_eq!(Model::menu_items(&working), [MenuItem::Focus]);
    }

    #[test]
    fn saved_state_round_trips_through_the_ledger() {
        let dir = std::env::temp_dir().join(crate::ledger::new_id());
        let ledger = crate::ledger::Ledger::new(dir.clone());
        assert_eq!(load(&ledger, "w1:p1"), None);
        let mut model = model();
        model.focus = Panel::Todos;
        model.fold_open = true;
        model.toggle_collapse(Panel::Agents);
        save(&ledger, "w1:p1", &model.saved());
        assert!(dir.join("sidebar").join("w1_p1.json").exists());
        let restored = Model::from_saved(load(&ledger, "w1:p1"));
        assert_eq!(restored.focus, Panel::Todos);
        assert!(restored.fold_open && restored.is_collapsed(Panel::Agents));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn total_counts_every_section() {
        let model = model();
        assert_eq!(
            (model.data.done(), model.data.dropped(), model.data.total()),
            (1, 1, 5)
        );
    }

    const GIB: u64 = 1024 * 1024 * 1024;

    fn limits() -> Thresholds {
        Thresholds {
            free_core_fraction_min: 0.25,
            memory_reserve_bytes: 3 * GIB,
            disk_free_min_percent: 15.0,
            reservation_load: 1.0,
            reservation_ttl_ms: 180_000,
        }
    }

    fn report(used_gib: u64, disk: f64, reachable: bool, eligible: bool) -> HostReport {
        serde_json::from_value(serde_json::json!({
            "id": "h", "label": "netcup", "target": "ssh", "service": if reachable { "up" } else { "down" },
            "reachable": reachable, "sampled_ms": 1_000, "error": null, "jobs": [],
            "probe": {
                "host": "h", "label": "netcup", "load1": 1.0, "cores": 16, "busy": 0.5,
                "memory": {"used_bytes": used_gib * GIB, "total_bytes": 48 * GIB},
                "disk_free_percent": disk,
                "agents": {"total": 0, "by_status": {}}, "agent_panes": [],
                "reservations": [{"id": "r", "created_ms": 0}], "panes": []
            },
            "headroom": {"host": "h", "headroom": 1.0, "eligible": eligible, "live_agents": 0}
        }))
        .unwrap()
    }

    #[test]
    fn host_rows_use_the_admission_thresholds() {
        let now = 1_000 + 2 * 60_000;
        // Two reservations' worth is held (one reservation plus the new agent): need 6 GiB.
        let ok = HostRow::from_report(&report(30, 40.0, true, true), &limits(), now);
        assert_eq!((ok.load.as_str(), ok.state), ("8/16", HostState::Eligible));
        assert_eq!(ok.memory.map(|m| m.1), Some(Level::Ok));
        assert_eq!(ok.disk.map(|d| d.1), Some(Level::Ok));
        // 48 - 41 = 7 GiB free: within 10% of the 6 GiB need is 6.6 GiB, so 7 is fine; 42 -> 6 GiB.
        let edge = HostRow::from_report(&report(41, 29.0, true, false), &limits(), now);
        assert_eq!(edge.memory.map(|m| m.1), Some(Level::Ok));
        assert_eq!(
            edge.disk.map(|d| d.1),
            Some(Level::Warn),
            "below 2x the 15% minimum"
        );
        assert_eq!(edge.state, HostState::NoHeadroom);
        let warn = HostRow::from_report(&report(41, 15.0, true, false), &limits(), now);
        assert_eq!(warn.disk.map(|d| d.1), Some(Level::Warn));
        let close = HostRow::from_report(&report(41, 14.9, true, false), &limits(), now);
        assert_eq!(
            close.disk.map(|d| d.1),
            Some(Level::Bad),
            "below the minimum: admission refuses"
        );
        // 48 - 41.5 = 6.5 GiB free, under 6.6 GiB but over 6: warn. 6 GiB exactly is not past it.
        let mut near = report(41, 40.0, true, false);
        near.probe.as_mut().unwrap().memory.used_bytes = 41 * GIB + GIB / 2;
        assert_eq!(
            HostRow::from_report(&near, &limits(), now)
                .memory
                .map(|m| m.1),
            Some(Level::Warn)
        );
        let mut past = report(41, 40.0, true, false);
        past.probe.as_mut().unwrap().memory.used_bytes = 43 * GIB;
        assert_eq!(
            HostRow::from_report(&past, &limits(), now)
                .memory
                .map(|m| m.1),
            Some(Level::Bad)
        );
    }

    #[test]
    fn an_unreachable_host_reports_how_long_it_has_been_silent() {
        let row = HostRow::from_report(
            &report(10, 40.0, false, false),
            &limits(),
            1_000 + 2 * 60_000,
        );
        assert_eq!(row.state, HostState::Unreachable);
        assert_eq!(row.silent_ms, Some(120_000));
        let mut data = Data::default();
        data.hosts.push(row);
        assert!(data.is_stale("netcup") && !data.is_stale("local"));
        let live = HostRow::from_report(&report(10, 40.0, true, true), &limits(), 5_000);
        assert_eq!(live.silent_ms, None);
    }

    #[test]
    fn tab_cycles_hosts_agents_todos() {
        let mut model = Model::default();
        let mut seen = vec![model.focus];
        for _ in 0..3 {
            model.focus_next();
            seen.push(model.focus);
        }
        assert_eq!(
            seen,
            [Panel::Agents, Panel::Todos, Panel::Hosts, Panel::Agents]
        );
        model.focus = Panel::Hosts;
        model.move_by(1);
        model.scroll_by(Panel::Hosts, 5);
        assert_eq!((model.card, model.scroll), (0, [0, 0]));
        model.toggle_collapse(Panel::Hosts);
        model.toggle_collapse(Panel::Todos);
        assert!(model.is_collapsed(Panel::Hosts) && model.is_collapsed(Panel::Todos));
        let restored = Model::from_saved(Some(model.saved()));
        assert!(restored.is_collapsed(Panel::Hosts) && restored.is_collapsed(Panel::Todos));
    }
}
