//! Draws the two framed panels and the footer. Pure: the same model and size give the same
//! cells. [`geometry`] also answers where things are, so input hit-tests what was drawn.

use super::model::{Card, HostRow, Level, Model, Panel, TodoRow, Tone};
use super::palette::Palette;
use crate::board::{TodoItem, TodoStatus};
use crate::commands::HostState;
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph, Widget},
};
use std::ops::Range;

/// Below this many columns the pill moves from a card's first line to its second.
pub const NARROW: u16 = 24;
/// Agents & Jobs takes at most this share of the height when both panels do not fit.
const AGENTS_SHARE: f32 = 0.6;
/// Hosts auto-collapses to one line per host when the other panels would get fewer rows.
const HOSTS_REST_MIN: u16 = 12;
/// Cells in a host's memory and disk meters.
const METER: usize = 8;
/// Nerd Font `nf-md-memory`.
const MEM_ICON: &str = "\u{f035b}";
/// Nerd Font `nf-md-harddisk`.
const DISK_ICON: &str = "\u{f02ca}";
/// Columns between the progress bar and its `n of m` label.
const BAR_GAP: usize = 5;

#[derive(Debug, Clone)]
pub struct PanelGeo {
    pub rect: Rect,
    pub collapsed: bool,
    /// Where the scrolling lines are drawn (inside the frame, above the pinned lines).
    pub body: Rect,
    pub lines: Vec<Line<'static>>,
    /// The first and last line of each selectable item.
    pub items: Vec<Range<usize>>,
    pub scroll: usize,
    pub title: String,
    pub count: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Geometry {
    pub hosts: PanelGeo,
    pub agents: PanelGeo,
    pub todos: PanelGeo,
    pub footer: Rect,
    pub close: Rect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    Close,
    /// The frame (title) line of a panel.
    Frame(Panel),
    Card(usize),
    Todo(usize),
    Empty(Panel),
}

impl Geometry {
    pub fn panel(&self, panel: Panel) -> &PanelGeo {
        match panel {
            Panel::Hosts => &self.hosts,
            Panel::Agents => &self.agents,
            Panel::Todos => &self.todos,
        }
    }

    pub fn hit(&self, x: u16, y: u16) -> Option<Hit> {
        if contains(self.close, x, y) {
            return Some(Hit::Close);
        }
        for panel in [Panel::Hosts, Panel::Agents, Panel::Todos] {
            let geo = self.panel(panel);
            if !contains(geo.rect, x, y) {
                continue;
            }
            if geo.collapsed || y == geo.rect.y || y + 1 == geo.rect.bottom() {
                return Some(Hit::Frame(panel));
            }
            if contains(geo.body, x, y) {
                let line = geo.scroll + usize::from(y - geo.body.y);
                if let Some(index) = geo.items.iter().position(|r| r.contains(&line)) {
                    return Some(match panel {
                        Panel::Agents => Hit::Card(index),
                        Panel::Todos => Hit::Todo(index),
                        Panel::Hosts => Hit::Empty(panel),
                    });
                }
            }
            return Some(Hit::Empty(panel));
        }
        None
    }
}

fn contains(rect: Rect, x: u16, y: u16) -> bool {
    x >= rect.x && x < rect.right() && y >= rect.y && y < rect.bottom()
}

// ── text helpers ────────────────────────────────────────────────────────────────────────

fn cells(text: &str) -> usize {
    Span::raw(text).width()
}

/// `text` clipped to `width` cells, ending in `…` when anything was cut.
pub fn fit(text: &str, width: usize) -> String {
    if cells(text) <= width {
        return text.to_owned();
    }
    if width == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut used = 0;
    for c in text.chars() {
        let w = cells(c.encode_utf8(&mut [0; 4]));
        if used + w + 1 > width {
            break;
        }
        out.push(c);
        used += w;
    }
    out.push('…');
    out
}

/// Greedy word wrap to `width` cells; a word wider than a line is split.
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut lines = Vec::new();
    for paragraph in text.lines() {
        let mut line = String::new();
        for word in paragraph.split_whitespace() {
            let mut word = word.to_owned();
            while cells(&word) > width {
                let mut head = String::new();
                for c in word.chars() {
                    if cells(&head) + cells(c.encode_utf8(&mut [0; 4])) > width {
                        break;
                    }
                    head.push(c);
                }
                if head.is_empty() {
                    break;
                }
                if !line.is_empty() {
                    lines.push(std::mem::take(&mut line));
                }
                word.drain(..head.len());
                lines.push(head);
            }
            if line.is_empty() {
                line = word;
            } else if cells(&line) + 1 + cells(&word) <= width {
                line.push(' ');
                line.push_str(&word);
            } else {
                lines.push(std::mem::replace(&mut line, word));
            }
        }
        if !line.is_empty() {
            lines.push(line);
        }
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

fn spaces(n: usize) -> Span<'static> {
    Span::raw(" ".repeat(n))
}

/// `left`, then `right` flush to the end of a `width`-cell line.
fn split(
    mut left: Vec<Span<'static>>,
    right: Vec<Span<'static>>,
    width: usize,
) -> Vec<Span<'static>> {
    let used: usize = left.iter().chain(&right).map(Span::width).sum();
    left.push(spaces(width.saturating_sub(used)));
    left.extend(right);
    left
}

fn line(spans: Vec<Span<'static>>, bg: Option<Color>) -> Line<'static> {
    let line = Line::from(spans);
    bg.map_or(line.clone(), |bg| line.style(Style::new().bg(bg)))
}

fn fg(color: Color) -> Style {
    Style::new().fg(color)
}

fn bold(color: Color) -> Style {
    Style::new().fg(color).add_modifier(Modifier::BOLD)
}

fn age(ms: u64) -> String {
    crate::watchdog::age(ms)
}

// ── hosts ───────────────────────────────────────────────────────────────────────────────

fn level_color(pal: &Palette, level: Level) -> Color {
    match level {
        Level::Ok => pal.green,
        Level::Warn => pal.yellow,
        Level::Bad => pal.red,
    }
}

fn host_color(pal: &Palette, state: HostState) -> Color {
    match state {
        HostState::Eligible => pal.green,
        HostState::NoHeadroom => pal.yellow,
        HostState::Unreachable => pal.red,
    }
}

/// The marker column: ` ▲` without headroom, ` ?` when unreachable or not yet measured.
fn host_marker(row: &HostRow) -> &'static str {
    if row.unreachable() || !row.measured {
        " ?"
    } else {
        match row.state {
            HostState::NoHeadroom => " ▲",
            _ => "  ",
        }
    }
}

fn host_lines(model: &Model, pal: &Palette, width: usize, compact: bool) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    for row in &model.data.hosts {
        let bar = Span::styled("▌", fg(host_color(pal, row.state)));
        let shown_load = if row.unreachable() || !row.measured {
            ""
        } else {
            row.load.as_str()
        };
        if compact {
            let mut text = row.label.clone();
            if !shown_load.is_empty() {
                text = format!("{text} {shown_load}");
            }
            let marker = host_marker(row);
            let room = width.saturating_sub(2 + marker.trim_end().len());
            out.push(Line::from(vec![
                bar,
                Span::raw(" "),
                Span::styled(fit(&text, room), bold(pal.text)),
                Span::styled(marker.trim_end().to_owned(), fg(pal.overlay1)),
            ]));
            continue;
        }
        // Load is right-aligned to a fixed column: marker slot (2) and a 2-cell margin follow.
        let right = vec![
            Span::styled(shown_load.to_owned(), fg(pal.overlay1)),
            Span::styled(host_marker(row).to_owned(), fg(pal.overlay1)),
            Span::raw("  "),
        ];
        let used: usize = right.iter().map(Span::width).sum();
        let first = vec![
            bar.clone(),
            Span::raw(" "),
            Span::styled(
                fit(&row.label, width.saturating_sub(2 + used + 1)),
                bold(pal.text),
            ),
        ];
        out.push(Line::from(split(first, right, width)));
        let mut second = vec![bar, Span::raw(" ")];
        second.extend(host_detail(row, pal, width.saturating_sub(3)));
        out.push(Line::from(second));
    }
    out
}

/// Line 2: the memory and disk meters, or why there is none. Longer forms give way to
/// shorter ones as the pane narrows.
fn host_detail(row: &HostRow, pal: &Palette, room: usize) -> Vec<Span<'static>> {
    if row.unreachable() {
        let text = row.silent_ms.map_or_else(
            || "no reply".to_owned(),
            |ms| format!("no reply {}", age(ms)),
        );
        return vec![Span::styled(fit(&text, room), fg(pal.overlay0))];
    }
    let (Some((used, memory)), Some((free, disk))) = (row.memory, row.disk) else {
        return vec![Span::styled(fit("no snapshot", room), fg(pal.overlay0))];
    };
    let mut line = meter(pal, MEM_ICON, used, memory, true);
    let mem_width: usize = line.iter().map(Span::width).sum();
    // The disk meter fills by the share of the disk in use, so it reads like the memory
    // meter. It gives up its percent first, and then goes away, as the pane narrows.
    for percent in [true, false] {
        let disk_meter = meter(pal, DISK_ICON, 1.0 - free / 100.0, disk, percent);
        let disk_width: usize = disk_meter.iter().map(Span::width).sum();
        if mem_width + 2 + disk_width <= room {
            line.push(Span::raw("  "));
            line.extend(disk_meter);
            break;
        }
    }
    line
}

/// An icon, a meter of `METER` cells filled to `fraction`, and the percent when asked for.
fn meter(
    pal: &Palette,
    icon: &str,
    fraction: f64,
    level: Level,
    percent: bool,
) -> Vec<Span<'static>> {
    let fraction = fraction.clamp(0.0, 1.0);
    let filled = ((fraction * METER as f64).round() as usize).min(METER);
    let mut spans = vec![
        Span::styled(format!("{icon} "), fg(pal.overlay1)),
        Span::styled("▰".repeat(filled), fg(level_color(pal, level))),
        Span::styled("▱".repeat(METER - filled), fg(pal.surface1)),
    ];
    if percent {
        spans.push(Span::styled(
            format!(" {:.0}%", fraction * 100.0),
            fg(pal.overlay1),
        ));
    }
    spans
}

// ── agents ──────────────────────────────────────────────────────────────────────────────

fn tone_color(pal: &Palette, tone: Tone) -> Color {
    match tone {
        Tone::ToLand => pal.teal,
        Tone::Alert => pal.red,
        Tone::Busy => pal.yellow,
        Tone::Idle => pal.green,
        Tone::Queued => pal.overlay0,
    }
}

fn agent_lines(
    model: &Model,
    pal: &Palette,
    width: usize,
    narrow: bool,
) -> (Vec<Line<'static>>, Vec<Range<usize>>) {
    let mut lines = Vec::new();
    let mut ranges = Vec::new();
    for (index, card) in model.data.cards.iter().enumerate() {
        if index > 0 {
            lines.push(Line::raw(""));
        }
        let start = lines.len();
        let selected = index == model.card;
        lines.extend(card_lines(model, pal, card, selected, width, narrow));
        ranges.push(start..lines.len());
    }
    (lines, ranges)
}

fn card_lines(
    model: &Model,
    pal: &Palette,
    card: &Card,
    selected: bool,
    width: usize,
    narrow: bool,
) -> Vec<Line<'static>> {
    let stale = model.data.is_stale(&card.host);
    let color = if stale {
        pal.overlay0
    } else {
        tone_color(pal, card.tone)
    };
    let bg = selected.then_some(pal.surface0);
    let bar = || Span::styled("▌", fg(color));
    let pill = Span::styled(
        format!(" {} ", card.pill),
        Style::new()
            .fg(pal.crust)
            .bg(color)
            .add_modifier(Modifier::BOLD),
    );
    let mut out = Vec::new();

    let mut first = vec![bar(), Span::raw(" ")];
    let mut second = vec![bar(), Span::raw(" ")];
    let name_room = width.saturating_sub(2 + if narrow { 0 } else { pill.width() + 1 });
    first.push(Span::styled(fit(&card.name, name_room), bold(pal.text)));
    let mut tail = Vec::new();
    if let Some(ahead) = card.ahead {
        tail.push(Span::styled(format!("+{ahead}"), bold(pal.green)));
        tail.push(Span::raw(" "));
    }
    tail.push(Span::styled(age(card.age_ms), fg(pal.overlay0)));
    tail.push(Span::raw(" "));
    let tail_width: usize = tail.iter().map(Span::width).sum();
    let sub_room = width.saturating_sub(2 + 1 + if narrow { pill.width() } else { tail_width });
    second.push(Span::styled(
        fit(&card.subtitle(), sub_room),
        fg(pal.overlay1),
    ));
    if narrow {
        out.push(line(first, bg));
        out.push(line(split(second, vec![pill], width), bg));
    } else {
        out.push(line(split(first, vec![pill], width), bg));
        out.push(line(split(second, tail, width), bg));
    }
    if selected {
        match &model.menu {
            Some(menu) => {
                if let Some(item) = menu.confirming {
                    let text = format!("{}? y/n", item.label());
                    out.push(line(
                        vec![
                            bar(),
                            Span::raw(" "),
                            Span::styled(fit(&text, width.saturating_sub(2)), bold(pal.yellow)),
                        ],
                        bg,
                    ));
                } else {
                    for (i, item) in menu.items.iter().enumerate() {
                        let on = i == menu.selected;
                        let shade = if on {
                            bold(pal.accent)
                        } else {
                            fg(pal.subtext0)
                        };
                        let text = format!("{} {}", if on { "▸" } else { " " }, item.label());
                        out.push(line(
                            vec![
                                bar(),
                                Span::raw(" "),
                                Span::styled(fit(&text, width.saturating_sub(2)), shade),
                            ],
                            bg,
                        ));
                    }
                }
            }
            None if !card.activity.is_empty() => {
                out.push(line(
                    vec![
                        bar(),
                        Span::raw(" "),
                        Span::styled(
                            fit(&card.activity, width.saturating_sub(2)),
                            fg(pal.subtext0),
                        ),
                    ],
                    bg,
                ));
            }
            None => {}
        }
    }
    out
}

// ── todos ───────────────────────────────────────────────────────────────────────────────

fn glyph(status: TodoStatus, pal: &Palette) -> (&'static str, Color) {
    match status {
        TodoStatus::InProgress => ("●", pal.accent),
        TodoStatus::Pending => ("○", pal.subtext0),
        TodoStatus::Completed => ("✓", pal.green),
        TodoStatus::Abandoned => ("−", pal.overlay0),
    }
}

struct Rows<'a> {
    model: &'a Model,
    pal: &'a Palette,
    width: usize,
    lines: Vec<Line<'static>>,
    items: Vec<Range<usize>>,
}

impl Rows<'_> {
    fn selected(&self, index: usize) -> bool {
        self.model.focus == Panel::Todos && self.model.todo == index
    }

    fn blank(&mut self) {
        self.lines.push(Line::raw(""));
    }

    fn header(&mut self, text: &str) {
        self.lines.push(Line::from(vec![
            Span::raw(" "),
            Span::styled(text.to_owned(), bold(self.pal.overlay0)),
        ]));
    }

    /// An in-progress row: `▌ ● headline`, its description under it, `surface0` behind it.
    fn now(&mut self, index: usize, row: TodoRow, item: &TodoItem) {
        let pal = *self.pal;
        let bg = if self.selected(index) {
            pal.surface1
        } else {
            pal.surface0
        };
        let expanded = self.model.expanded.contains(&row);
        let text_room = self.width.saturating_sub(5);
        let bar = || Span::styled("▌", fg(pal.accent));
        let head = |dot: bool, text: String| {
            vec![
                bar(),
                Span::raw(" "),
                if dot {
                    Span::styled("●", fg(pal.accent))
                } else {
                    Span::raw(" ")
                },
                Span::raw(" "),
                Span::styled(text, bold(pal.text)),
            ]
        };
        let cont =
            |text: String, style: Style| vec![bar(), Span::raw("   "), Span::styled(text, style)];
        let start = self.lines.len();
        let described = item.description.as_deref().filter(|d| !d.trim().is_empty());
        if expanded || described.is_none() {
            for (i, part) in wrap(&item.text, text_room).into_iter().enumerate() {
                self.lines.push(line(
                    if i == 0 {
                        head(true, part)
                    } else {
                        cont(part, bold(pal.text))
                    },
                    Some(bg),
                ));
            }
        } else {
            self.lines.push(line(
                head(true, fit(&item.text, self.width.saturating_sub(4))),
                Some(bg),
            ));
        }
        if let Some(description) = described {
            for part in wrap(description, text_room) {
                self.lines
                    .push(line(cont(part, fg(pal.overlay1)), Some(bg)));
            }
        }
        if expanded {
            for child in &item.children {
                let (mark, color) = glyph(child.status, &pal);
                for (i, part) in wrap(&child.text, text_room.saturating_sub(2))
                    .into_iter()
                    .enumerate()
                {
                    let lead = if i == 0 {
                        Span::styled(mark, fg(color))
                    } else {
                        Span::raw(" ")
                    };
                    self.lines.push(line(
                        vec![
                            bar(),
                            Span::raw("   "),
                            lead,
                            Span::raw(" "),
                            Span::styled(part, fg(pal.subtext0)),
                        ],
                        Some(bg),
                    ));
                }
            }
        }
        self.items.push(start..self.lines.len());
    }

    /// A pending (NEXT) row, or a finished one under the open fold.
    fn plain(&mut self, index: usize, row: TodoRow, item: &TodoItem, indent: usize) {
        let pal = *self.pal;
        let bg = self.selected(index).then_some(pal.surface0);
        let expanded = self.model.expanded.contains(&row);
        let (mark, color) = glyph(item.status, &pal);
        let (mark_color, text_color) = if item.status == TodoStatus::Pending {
            (pal.subtext0, pal.subtext0)
        } else {
            (color, pal.overlay0)
        };
        let lead = 1 + indent;
        let room = self.width.saturating_sub(lead + 2);
        let start = self.lines.len();
        let mark_span = || Span::styled(mark, fg(mark_color));
        if expanded {
            for (i, part) in wrap(&item.text, room).into_iter().enumerate() {
                let head = if i == 0 { mark_span() } else { Span::raw(" ") };
                self.lines.push(line(
                    vec![
                        spaces(lead),
                        head,
                        Span::raw(" "),
                        Span::styled(part, fg(text_color)),
                    ],
                    bg,
                ));
            }
            if let Some(description) = item.description.as_deref().filter(|d| !d.trim().is_empty())
            {
                for part in wrap(description, room) {
                    self.lines.push(line(
                        vec![spaces(lead + 2), Span::styled(part, fg(pal.overlay1))],
                        bg,
                    ));
                }
            }
            for child in &item.children {
                let (child_mark, child_color) = glyph(child.status, &pal);
                for (i, part) in wrap(&child.text, room.saturating_sub(2))
                    .into_iter()
                    .enumerate()
                {
                    let head = if i == 0 {
                        Span::styled(child_mark, fg(child_color))
                    } else {
                        Span::raw(" ")
                    };
                    self.lines.push(line(
                        vec![
                            spaces(lead + 2),
                            head,
                            Span::raw(" "),
                            Span::styled(part, fg(pal.subtext0)),
                        ],
                        bg,
                    ));
                }
            }
        } else {
            self.lines.push(line(
                vec![
                    spaces(lead),
                    mark_span(),
                    Span::raw(" "),
                    Span::styled(fit(&item.text, room), fg(text_color)),
                ],
                bg,
            ));
        }
        self.items.push(start..self.lines.len());
    }

    fn fold(&mut self, index: usize) {
        let pal = *self.pal;
        let data = &self.model.data;
        let bg = self.selected(index).then_some(pal.surface0);
        let mut left = vec![Span::raw(" ")];
        if data.done() > 0 {
            left.push(Span::styled(
                format!("✓ {} done", data.done()),
                fg(pal.green),
            ));
        }
        if data.done() > 0 && data.dropped() > 0 {
            left.push(Span::raw("   "));
        }
        if data.dropped() > 0 {
            left.push(Span::styled(
                format!("− {} dropped", data.dropped()),
                fg(pal.overlay0),
            ));
        }
        let arrow = if self.model.fold_open { "▾" } else { "▸" };
        let start = self.lines.len();
        self.lines.push(line(
            split(
                left,
                vec![Span::styled(arrow, fg(pal.overlay0)), Span::raw(" ")],
                self.width,
            ),
            bg,
        ));
        self.items.push(start..self.lines.len());
    }
}

fn todo_lines(
    model: &Model,
    pal: &Palette,
    width: usize,
) -> (Vec<Line<'static>>, Vec<Range<usize>>) {
    let data = &model.data;
    let Some(todos) = data.todos.as_ref().filter(|_| data.total() > 0) else {
        return (Vec::new(), Vec::new());
    };
    let mut rows = Rows {
        model,
        pal,
        width,
        lines: Vec::new(),
        items: Vec::new(),
    };
    // Progress: green for done, surface1 for the rest.
    let label = format!("{} of {}", data.done(), data.total());
    let gap = if width >= 30 { BAR_GAP } else { 1 };
    let bar = width.saturating_sub(2 + cells(&label) + gap).max(1);
    let filled = (bar * data.done()).div_ceil(data.total().max(1)).min(bar);
    let filled = if data.done() == 0 { 0 } else { filled };
    rows.lines.push(Line::from(vec![
        Span::raw(" "),
        Span::styled("━".repeat(filled), fg(pal.green)),
        Span::styled("━".repeat(bar - filled), fg(pal.surface1)),
        spaces(width.saturating_sub(2 + bar + cells(&label))),
        Span::styled(label, fg(pal.overlay0)),
        Span::raw(" "),
    ]));
    let mut index = 0;
    let (now, next) = (&todos.sections[0].items, &todos.sections[1].items);
    if !now.is_empty() {
        rows.blank();
        rows.header("NOW");
        for (i, item) in now.iter().enumerate() {
            rows.now(index, TodoRow::Item(0, i), item);
            index += 1;
        }
    }
    if !next.is_empty() {
        rows.blank();
        rows.header("NEXT");
        for (i, item) in next.iter().enumerate() {
            rows.plain(index, TodoRow::Item(1, i), item, 0);
            index += 1;
        }
    }
    if data.done() + data.dropped() > 0 {
        rows.blank();
        rows.fold(index);
        index += 1;
        if model.fold_open {
            for section in [2, 3] {
                for (i, item) in todos.sections[section].items.iter().enumerate() {
                    rows.plain(index, TodoRow::Closed(section, i), item, 2);
                    index += 1;
                }
            }
        }
    }
    (rows.lines, rows.items)
}

// ── geometry ────────────────────────────────────────────────────────────────────────────

fn panel_geo(
    model: &Model,
    panel: Panel,
    rect: Rect,
    collapsed: bool,
    lines: Vec<Line<'static>>,
    items: Vec<Range<usize>>,
) -> PanelGeo {
    let data = &model.data;
    let (title, count) = match panel {
        Panel::Hosts => ("Hosts", Some(data.hosts.len().to_string())),
        Panel::Agents => ("Agents & Jobs", Some(data.cards.len().to_string())),
        Panel::Todos => (
            "Todos",
            (data.total() > 0).then(|| format!("{}/{}", data.done(), data.total())),
        ),
    };
    let inner_h = usize::from(rect.height.saturating_sub(2));
    let view_h = inner_h;
    let body = Rect::new(
        rect.x + 1,
        rect.y + 1,
        rect.width.saturating_sub(2),
        u16::try_from(view_h).unwrap_or(u16::MAX),
    );
    let max = lines.len().saturating_sub(view_h);
    // Hosts never scrolls and has nothing to select.
    let (slot, selected) = match panel {
        Panel::Hosts => (None, 0),
        Panel::Agents => (Some(0), model.card),
        Panel::Todos => (Some(1), model.todo),
    };
    let mut scroll = slot.map_or(0, |slot| model.scroll[slot]).min(max);
    if model.follow
        && let Some(range) = items.get(selected)
    {
        if range.start < scroll {
            scroll = range.start;
        } else if range.end > scroll + view_h {
            scroll = (range.end.saturating_sub(view_h)).min(range.start);
        }
        scroll = scroll.min(max);
    }
    PanelGeo {
        rect,
        collapsed,
        body,
        lines,
        items,
        scroll,
        title: title.to_owned(),
        count,
    }
}

/// Lay out the panels in `area` (the pane): Hosts first at a fixed height, then the 60% rule
/// and collapse for Agents & Jobs and Todos.
pub fn geometry(model: &Model, area: Rect, pal: &Palette) -> Geometry {
    let width = usize::from(area.width.saturating_sub(2));
    let narrow = area.width < NARROW;
    let h = |n: usize| u16::try_from(n).unwrap_or(u16::MAX);
    let avail = area.height.saturating_sub(1);

    // Hosts: two lines per host, or one when folded by `space` or when the pane is short.
    let hosts = model.data.hosts.len();
    let full_h = h(2 + (2 * hosts).max(1));
    let compact_h = h(2 + hosts.max(1));
    let compact = model.hosts_compact || avail.saturating_sub(full_h) < HOSTS_REST_MIN;
    let hosts_h = (if compact { compact_h } else { full_h }).min(avail);
    let rest = avail - hosts_h;
    let host_lines = host_lines(model, pal, width, compact);

    let (agent_lines, agent_items) = agent_lines(model, pal, width, narrow);
    let (todo_lines, todo_items) = todo_lines(model, pal, width);
    let want_agents = h(2 + agent_lines.len().max(1));
    let want_todos = h(2 + todo_lines.len().max(1));
    let (collapsed_a, collapsed_t) = (
        model.is_collapsed(Panel::Agents),
        model.is_collapsed(Panel::Todos),
    );
    let (ha, ht) = match (collapsed_a, collapsed_t) {
        (true, _) => (1.min(rest), want_todos.min(rest.saturating_sub(1))),
        (_, true) => (want_agents.min(rest.saturating_sub(1)), 1.min(rest)),
        _ if want_agents + want_todos <= rest => (want_agents, want_todos),
        _ => {
            let cap = (f32::from(rest) * AGENTS_SHARE).floor() as u16;
            let first = want_agents.min(cap.max(3).min(rest));
            let t = want_todos.min(rest - first);
            (want_agents.min(rest - t), t)
        }
    };
    let hosts_rect = Rect::new(area.x, area.y, area.width, hosts_h);
    let agents_rect = Rect::new(area.x, area.y + hosts_h, area.width, ha);
    let todos_rect = Rect::new(area.x, area.y + hosts_h + ha, area.width, ht);
    let footer = Rect::new(
        area.x,
        area.y + area.height.saturating_sub(1),
        area.width,
        area.height.min(1),
    );
    let close = Rect::new(
        footer.right().saturating_sub(3),
        footer.y,
        footer.width.min(3),
        footer.height,
    );
    Geometry {
        hosts: panel_geo(
            model,
            Panel::Hosts,
            hosts_rect,
            false,
            host_lines,
            Vec::new(),
        ),
        agents: panel_geo(
            model,
            Panel::Agents,
            agents_rect,
            collapsed_a,
            agent_lines,
            agent_items,
        ),
        todos: panel_geo(
            model,
            Panel::Todos,
            todos_rect,
            collapsed_t,
            todo_lines,
            todo_items,
        ),
        footer,
        close,
    }
}

/// Persist the scroll the last draw settled on, so the next one starts from it.
pub fn settle(model: &mut Model, geo: &Geometry) {
    model.scroll = [geo.agents.scroll, geo.todos.scroll];
}

// ── drawing ─────────────────────────────────────────────────────────────────────────────

fn title_line(text: &str, pal: &Palette) -> Line<'static> {
    Line::from(Span::styled(format!(" {text} "), bold(pal.text)))
}

fn count_line(text: &str, pal: &Palette) -> Line<'static> {
    Line::from(Span::styled(format!(" {text} "), bold(pal.overlay0))).right_aligned()
}

fn render_panel(
    model: &Model,
    pal: &Palette,
    panel: Panel,
    geo: &PanelGeo,
    buf: &mut Buffer,
    empty: &str,
) {
    let rect = geo.rect;
    if rect.height == 0 || rect.width < 2 {
        return;
    }
    let border_color = if model.focus == panel {
        pal.accent
    } else {
        pal.surface1
    };
    let mut block = Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(fg(border_color))
        .title(title_line(&geo.title, pal));
    if let Some(count) = &geo.count {
        block = block.title(count_line(count, pal));
    }
    if geo.collapsed {
        // Only the top border: `╭ Todos ──── 3/7 ╮`.
        let frame = Rect::new(rect.x, rect.y, rect.width, 1);
        let count = geo.count.as_ref().map(|c| format!(" {c} "));
        let title = format!(" {} ", geo.title);
        let fill = usize::from(frame.width)
            .saturating_sub(2 + cells(&title) + count.as_ref().map_or(0, |c| cells(c)));
        let mut spans = vec![
            Span::styled("╭", fg(border_color)),
            Span::styled(title, bold(pal.text)),
            Span::styled("─".repeat(fill), fg(border_color)),
        ];
        if let Some(count) = count {
            spans.push(Span::styled(count, bold(pal.overlay0)));
        }
        spans.push(Span::styled("╮", fg(border_color)));
        Paragraph::new(Line::from(spans)).render(frame, buf);
        return;
    }
    block.render(rect, buf);
    let inner = Rect::new(
        rect.x + 1,
        rect.y + 1,
        rect.width.saturating_sub(2),
        rect.height.saturating_sub(2),
    );
    if geo.lines.is_empty() {
        let text = Line::from(Span::styled(empty.to_owned(), fg(pal.overlay0))).centered();
        Paragraph::new(text).render(
            Rect::new(inner.x, inner.y, inner.width, inner.height.min(1)),
            buf,
        );
    } else {
        let visible: Vec<Line<'static>> = geo
            .lines
            .iter()
            .skip(geo.scroll)
            .take(usize::from(geo.body.height))
            .cloned()
            .collect();
        // A line's background runs edge to edge, not just under its text.
        for (offset, line) in (0..).zip(&visible) {
            buf.set_style(
                Rect::new(geo.body.x, geo.body.y + offset, geo.body.width, 1),
                line.style,
            );
        }
        Paragraph::new(visible).render(geo.body, buf);
    }
}

fn footer_text(model: &Model) -> String {
    if let Some(notice) = &model.notice {
        return format!(" {notice}");
    }
    match &model.menu {
        Some(menu) if menu.confirming.is_some() => " y confirm  n cancel".into(),
        Some(_) => " ↑↓ move  ⏎ pick  esc back".into(),
        None => " ⏎ focus  tab panel  m menu".into(),
    }
}

pub fn render(model: &Model, area: Rect, buf: &mut Buffer, pal: &Palette) {
    let geo = geometry(model, area, pal);
    render_panel(model, pal, Panel::Hosts, &geo.hosts, buf, "No hosts");
    render_panel(
        model,
        pal,
        Panel::Agents,
        &geo.agents,
        buf,
        "No agents or jobs",
    );
    render_panel(
        model,
        pal,
        Panel::Todos,
        &geo.todos,
        buf,
        "No todos for this session",
    );
    let footer = geo.footer;
    if footer.height == 0 {
        return;
    }
    let width = usize::from(footer.width);
    let text = fit(&footer_text(model), width.saturating_sub(4));
    let spans = split(
        vec![Span::styled(text, fg(pal.overlay0))],
        vec![Span::styled("« ", fg(pal.overlay0))],
        width,
    );
    Paragraph::new(Line::from(spans)).render(footer, buf);
}

#[cfg(test)]
mod tests;
