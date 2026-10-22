#![allow(clippy::many_single_char_names)]
use super::*;
use crate::board::{Group, TodoStatus};
use crate::commands::HostState;
use crate::sidebar::model::tests::{item, todos};
use crate::sidebar::model::{Card, Data, HostRow, Level, Model, Tone};
use ratatui::{Terminal, backend::TestBackend};

const MIN: u64 = 60_000;

fn host(label: &str, load: &str, state: HostState, mem: f64, disk: f64) -> HostRow {
    HostRow {
        label: label.into(),
        load: load.into(),
        state,
        measured: true,
        memory: Some((mem, Level::Ok)),
        disk: Some((disk, Level::Ok)),
        silent_ms: None,
    }
}

fn hosts() -> Vec<HostRow> {
    vec![
        host("local", "11/12", HostState::NoHeadroom, 0.76, 22.0),
        host("netcup", "14/16", HostState::NoHeadroom, 0.65, 24.0),
        host("momokaya-2", "2/7", HostState::Eligible, 0.55, 39.0),
    ]
}

#[allow(clippy::too_many_arguments)]
fn card(
    id: &str,
    kind: &str,
    harness: &str,
    host: &str,
    pill: &str,
    tone: Tone,
    group: Group,
    age: u64,
) -> Card {
    Card {
        id: id.into(),
        kind: kind.into(),
        name: id.into(),
        harness: harness.into(),
        host: host.into(),
        pane: Some(format!("p-{id}")),
        group,
        pill: pill.into(),
        tone,
        ahead: None,
        age_ms: age * MIN,
        activity: String::new(),
    }
}

fn cards() -> Vec<Card> {
    let mut web = card(
        "web-tests",
        "agent",
        "pi",
        "netcup",
        "to land",
        Tone::ToLand,
        Group::ToLand,
        3,
    );
    web.ahead = Some(2);
    let i18n = card(
        "i18n-catalog",
        "agent",
        "pi",
        "netcup",
        "stalled",
        Tone::Alert,
        Group::Attention,
        22,
    );
    let mut api = card(
        "api-dups",
        "agent",
        "pi",
        "momokaya-2",
        "working",
        Tone::Busy,
        Group::Running,
        14,
    );
    api.activity = "edit src/api/dups.rs".into();
    let job = card(
        "cargo test --workspace",
        "job",
        "job",
        "local",
        "running",
        Tone::Busy,
        Group::Running,
        4,
    );
    vec![web, i18n, api, job]
}

fn todo_data() -> crate::board::TodoSections {
    todos([
        vec![item(
            TodoStatus::InProgress,
            "Land PR #112",
            Some("Release it, then update the installed Herdr plugin."),
        )],
        vec![
            item(TodoStatus::Pending, "Verify the sidebar on netcup", None),
            item(
                TodoStatus::Pending,
                "Update the dogsled skill and Rust docs",
                None,
            ),
        ],
        vec![
            item(TodoStatus::Completed, "Dock the board", None),
            item(TodoStatus::Completed, "Add the toggle", None),
            item(TodoStatus::Completed, "Pick a palette", None),
        ],
        vec![item(TodoStatus::Abandoned, "Plugin sidebar API", None)],
    ])
}

fn fixture() -> Model {
    Model {
        data: Data {
            cards: cards(),
            todos: Some(todo_data()),
            hosts: hosts(),
        },
        card: 2,
        ..Model::default()
    }
}

fn draw(model: &Model, width: u16, height: u16, pal: &Palette) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal
        .draw(|frame| render(model, frame.area(), frame.buffer_mut(), pal))
        .unwrap();
    terminal.backend().buffer().clone()
}

fn rows(buf: &Buffer) -> Vec<String> {
    (0..buf.area.height)
        .map(|y| {
            (0..buf.area.width)
                .map(|x| buf[(x, y)].symbol().to_owned())
                .collect::<String>()
        })
        .collect()
}

/// Every row, cell for cell. Trailing blank cells are not written out in the expected text.
fn assert_rows(buf: &Buffer, expected: &str) {
    let got = rows(buf)
        .iter()
        .map(|row| row.trim_end().to_owned())
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(got, expected.trim_matches('\n'), "\n{got}\n");
}

/// `(fg, bg, bold)` of a cell.
fn style(buf: &Buffer, x: u16, y: u16) -> (Color, Color, bool) {
    let cell = &buf[(x, y)];
    (cell.fg, cell.bg, cell.modifier.contains(Modifier::BOLD))
}

/// The cell where `needle` starts (every symbol drawn here is one cell wide).
fn find(buf: &Buffer, needle: &str) -> (u16, u16) {
    for (y, row) in rows(buf).iter().enumerate() {
        if let Some(byte) = row.find(needle) {
            return (
                u16::try_from(row[..byte].chars().count()).unwrap(),
                u16::try_from(y).unwrap(),
            );
        }
    }
    panic!("{needle:?} not drawn:\n{}", rows(buf).join("\n"));
}

fn row_of(buf: &Buffer, needle: &str) -> u16 {
    find(buf, needle).1
}

/// The cell where `needle` starts on row `y`.
fn find_on(buf: &Buffer, needle: &str, y: u16) -> u16 {
    let row = &rows(buf)[usize::from(y)];
    match row.find(needle) {
        Some(byte) => u16::try_from(row[..byte].chars().count()).unwrap(),
        None => panic!("{needle:?} not drawn on row {y}:\n{}", rows(buf).join("\n")),
    }
}

const FULL: &str = "\
╭ Hosts ──────────────────────── 3 ╮
│▌ local                  11/12 ▲  │
│▌ \u{f035b} ▰▰▰▰▰▰▱▱ 76%  \u{f02ca} ▰▰▰▰▰▰▱▱ 78%  │
│▌ netcup                 14/16 ▲  │
│▌ \u{f035b} ▰▰▰▰▰▱▱▱ 65%  \u{f02ca} ▰▰▰▰▰▰▱▱ 76%  │
│▌ momokaya-2               2/7    │
│▌ \u{f035b} ▰▰▰▰▱▱▱▱ 55%  \u{f02ca} ▰▰▰▰▰▱▱▱ 61%  │
╰──────────────────────────────────╯
╭ Agents & Jobs ──────────────── 4 ╮
│▌ web-tests               to land │
│▌ pi · netcup               +2 3m │
│                                  │
│▌ i18n-catalog            stalled │
│▌ pi · netcup                 22m │
│                                  │
│▌ api-dups                working │
│▌ pi · momokaya-2             14m │
│▌ edit src/api/dups.rs            │
│                                  │
│▌ cargo test --workspace  running │
│▌ job · local                  4m │
╰──────────────────────────────────╯
╭ Todos ────────────────────── 3/7 ╮
│ ━━━━━━━━━━━━━━━━━━━━━     3 of 7 │
│                                  │
│ NOW                              │
│▌ ● Land PR #112                  │
│▌   Release it, then update the   │
│▌   installed Herdr plugin.       │
│                                  │
│ NEXT                             │
│ ○ Verify the sidebar on netcup   │
│ ○ Update the dogsled skill and R…│
│                                  │
│ ✓ 3 done   − 1 dropped         ▸ │
╰──────────────────────────────────╯
 ⏎ focus  tab panel  m menu       «
";

const SELECTED: &str = "\
╭ Hosts ──────────────────────── 3 ╮
│▌ local                  11/12 ▲  │
│▌ \u{f035b} ▰▰▰▰▰▰▱▱ 76%  \u{f02ca} ▰▰▰▰▰▰▱▱ 78%  │
│▌ netcup                 14/16 ▲  │
│▌ \u{f035b} ▰▰▰▰▰▱▱▱ 65%  \u{f02ca} ▰▰▰▰▰▰▱▱ 76%  │
│▌ momokaya-2               2/7    │
│▌ \u{f035b} ▰▰▰▰▱▱▱▱ 55%  \u{f02ca} ▰▰▰▰▰▱▱▱ 61%  │
╰──────────────────────────────────╯
╭ Agents & Jobs ──────────────── 4 ╮
│▌ web-tests               to land │
│▌ pi · netcup               +2 3m │
│                                  │
│▌ i18n-catalog            stalled │
│▌ pi · netcup                 22m │
│                                  │
│▌ api-dups                working │
│▌ pi · momokaya-2             14m │
│                                  │
│▌ cargo test --workspace  running │
│▌ job · local                  4m │
╰──────────────────────────────────╯
╭ Todos ────────────────────── 3/7 ╮
│ ━━━━━━━━━━━━━━━━━━━━━     3 of 7 │
│                                  │
│ NOW                              │
│▌ ● Land PR #112                  │
│▌   Release it, then update the   │
│▌   installed Herdr plugin.       │
│                                  │
│ NEXT                             │
│ ○ Verify the sidebar on netcup   │
│ ○ Update the dogsled skill and R…│
│                                  │
│ ✓ 3 done   − 1 dropped         ▸ │
╰──────────────────────────────────╯

 ⏎ focus  tab panel  m menu       «
";

const TODOS_COLLAPSED: &str = "\
╭ Hosts ──────────────────────── 3 ╮
│▌ local                  11/12 ▲  │
│▌ \u{f035b} ▰▰▰▰▰▰▱▱ 76%  \u{f02ca} ▰▰▰▰▰▰▱▱ 78%  │
│▌ netcup                 14/16 ▲  │
│▌ \u{f035b} ▰▰▰▰▰▱▱▱ 65%  \u{f02ca} ▰▰▰▰▰▰▱▱ 76%  │
│▌ momokaya-2               2/7    │
│▌ \u{f035b} ▰▰▰▰▱▱▱▱ 55%  \u{f02ca} ▰▰▰▰▰▱▱▱ 61%  │
╰──────────────────────────────────╯
╭ Agents & Jobs ──────────────── 4 ╮
│▌ web-tests               to land │
│▌ pi · netcup               +2 3m │
│                                  │
│▌ i18n-catalog            stalled │
│▌ pi · netcup                 22m │
│                                  │
│▌ api-dups                working │
│▌ pi · momokaya-2             14m │
│▌ edit src/api/dups.rs            │
│                                  │
│▌ cargo test --workspace  running │
│▌ job · local                  4m │
╰──────────────────────────────────╯
╭ Todos ────────────────────── 3/7 ╮













 ⏎ focus  tab panel  m menu       «
";

const EXPANDED: &str = "\
╭ Hosts ──────────────────────── 3 ╮
│▌ local                  11/12 ▲  │
│▌ \u{f035b} ▰▰▰▰▰▰▱▱ 76%  \u{f02ca} ▰▰▰▰▰▰▱▱ 78%  │
│▌ netcup                 14/16 ▲  │
│▌ \u{f035b} ▰▰▰▰▰▱▱▱ 65%  \u{f02ca} ▰▰▰▰▰▰▱▱ 76%  │
│▌ momokaya-2               2/7    │
│▌ \u{f035b} ▰▰▰▰▱▱▱▱ 55%  \u{f02ca} ▰▰▰▰▰▱▱▱ 61%  │
╰──────────────────────────────────╯
╭ Agents & Jobs ──────────────── 4 ╮
│▌ web-tests               to land │
│▌ pi · netcup               +2 3m │
│                                  │
│▌ i18n-catalog            stalled │
│▌ pi · netcup                 22m │
│                                  │
│▌ api-dups                working │
│▌ pi · momokaya-2             14m │
│▌ edit src/api/dups.rs            │
│                                  │
│▌ cargo test --workspace  running │
│▌ job · local                  4m │
╰──────────────────────────────────╯
╭ Todos ────────────────────── 3/7 ╮
│ ━━━━━━━━━━━━━━━━━━━━━     3 of 7 │
│                                  │
│ NOW                              │
│▌ ● Land PR #112                  │
│▌   Release it, then update the   │
│▌   installed Herdr plugin.       │
│                                  │
│ NEXT                             │
│ ○ Verify the sidebar on netcup   │
│ ○ Update the dogsled skill and R…│
│                                  │
│ ✓ 3 done   − 1 dropped         ▾ │
│   ✓ Dock the board               │
│   ✓ Add the toggle               │
│   ✓ Pick a palette               │
│   − Plugin sidebar API           │
╰──────────────────────────────────╯
 ⏎ focus  tab panel  m menu       «
";

const EMPTY: &str = "\
╭ Hosts ──────────────────────── 3 ╮
│▌ local                  11/12 ▲  │
│▌ \u{f035b} ▰▰▰▰▰▰▱▱ 76%  \u{f02ca} ▰▰▰▰▰▰▱▱ 78%  │
│▌ netcup                 14/16 ▲  │
│▌ \u{f035b} ▰▰▰▰▰▱▱▱ 65%  \u{f02ca} ▰▰▰▰▰▰▱▱ 76%  │
│▌ momokaya-2               2/7    │
│▌ \u{f035b} ▰▰▰▰▱▱▱▱ 55%  \u{f02ca} ▰▰▰▰▰▱▱▱ 61%  │
╰──────────────────────────────────╯
╭ Agents & Jobs ──────────────── 0 ╮
│         No agents or jobs        │
╰──────────────────────────────────╯
╭ Todos ───────────────────────────╮
│     No todos for this session    │
╰──────────────────────────────────╯













 ⏎ focus  tab panel  m menu       «
";

const STALE: &str = "\
╭ Hosts ──────────────────────── 3 ╮
│▌ local                  11/12 ▲  │
│▌ \u{f035b} ▰▰▰▰▰▰▱▱ 76%  \u{f02ca} ▰▰▰▰▰▰▱▱ 78%  │
│▌ netcup                       ?  │
│▌ no reply 2m                     │
│▌ momokaya-2               2/7    │
│▌ \u{f035b} ▰▰▰▰▱▱▱▱ 55%  \u{f02ca} ▰▰▰▰▰▱▱▱ 61%  │
╰──────────────────────────────────╯
╭ Agents & Jobs ──────────────── 4 ╮
│▌ web-tests               to land │
│▌ pi · netcup               +2 3m │
│                                  │
│▌ i18n-catalog            stalled │
│▌ pi · netcup                 22m │
│                                  │
│▌ api-dups                working │
│▌ pi · momokaya-2             14m │
│▌ edit src/api/dups.rs            │
│                                  │
│▌ cargo test --workspace  running │
│▌ job · local                  4m │
╰──────────────────────────────────╯
╭ Todos ────────────────────── 3/7 ╮
│ ━━━━━━━━━━━━━━━━━━━━━     3 of 7 │
│                                  │
│ NOW                              │
│▌ ● Land PR #112                  │
│▌   Release it, then update the   │
│▌   installed Herdr plugin.       │
│                                  │
│ NEXT                             │
│ ○ Verify the sidebar on netcup   │
│ ○ Update the dogsled skill and R…│
│                                  │
│ ✓ 3 done   − 1 dropped         ▸ │
╰──────────────────────────────────╯
 ⏎ focus  tab panel  m menu       «
";

const HOSTS_COLLAPSED: &str = "\
╭ Hosts ──────────────────────── 3 ╮
│▌ local 11/12 ▲                   │
│▌ netcup 14/16 ▲                  │
│▌ momokaya-2 2/7                  │
╰──────────────────────────────────╯
╭ Agents & Jobs ──────────────── 4 ╮
│▌ web-tests               to land │
│▌ pi · netcup               +2 3m │
│                                  │
│▌ i18n-catalog            stalled │
│▌ pi · netcup                 22m │
│                                  │
│▌ api-dups                working │
│▌ pi · momokaya-2             14m │
│▌ edit src/api/dups.rs            │
│                                  │
│▌ cargo test --workspace  running │
│▌ job · local                  4m │
╰──────────────────────────────────╯
╭ Todos ────────────────────── 3/7 ╮
│ ━━━━━━━━━━━━━━━━━━━━━     3 of 7 │
│                                  │
│ NOW                              │
│▌ ● Land PR #112                  │
│▌   Release it, then update the   │
│▌   installed Herdr plugin.       │
│                                  │
│ NEXT                             │
│ ○ Verify the sidebar on netcup   │
│ ○ Update the dogsled skill and R…│
│                                  │
│ ✓ 3 done   − 1 dropped         ▸ │
╰──────────────────────────────────╯
 ⏎ focus  tab panel  m menu       «
";

const AUTO_COLLAPSED: &str = "\
╭ Hosts ──────────────────────── 3 ╮
│▌ local 11/12 ▲                   │
│▌ netcup 14/16 ▲                  │
│▌ momokaya-2 2/7                  │
╰──────────────────────────────────╯
╭ Agents & Jobs ──────────────── 4 ╮
│▌ pi · netcup                 22m │
│                                  │
│▌ api-dups                working │
│▌ pi · momokaya-2             14m │
│▌ edit src/api/dups.rs            │
╰──────────────────────────────────╯
╭ Todos ────────────────────── 3/7 ╮
│ NOW                              │
│▌ ● Land PR #112                  │
│▌   Release it, then update the   │
│▌   installed Herdr plugin.       │
╰──────────────────────────────────╯
 ⏎ focus  tab panel  m menu       «
";

const W24: &str = "\
╭ Hosts ──────────── 3 ╮
│▌ local      11/12 ▲  │
│▌ \u{f035b} ▰▰▰▰▰▰▱▱ 76%      │
│▌ netcup     14/16 ▲  │
│▌ \u{f035b} ▰▰▰▰▰▱▱▱ 65%      │
│▌ momokaya-2   2/7    │
│▌ \u{f035b} ▰▰▰▰▱▱▱▱ 55%      │
╰──────────────────────╯
╭ Agents & Jobs ──── 4 ╮
│▌ web-tests   to land │
│▌ pi · netcup   +2 3m │
│                      │
│▌ i18n-cata…  stalled │
│▌ pi · netcup     22m │
│                      │
│▌ api-dups    working │
│▌ pi · momokaya-2 14m │
│▌ edit src/api/dups.rs│
│                      │
│▌ cargo tes…  running │
│▌ job · local      4m │
╰──────────────────────╯
╭ Todos ────────── 3/7 ╮
│ ━━━━━━━━━━━━━ 3 of 7 │
│                      │
│ NOW                  │
│▌ ● Land PR #112      │
│▌   Release it, then  │
│▌   update the        │
│▌   installed Herdr   │
│▌   plugin.           │
│                      │
│ NEXT                 │
│ ○ Verify the sidebar…│
│ ○ Update the dogsled…│
╰──────────────────────╯
 ⏎ focus  tab panel…  «
";

const W23: &str = "\
╭ Hosts ─────────── 3 ╮
│▌ local     11/12 ▲  │
│▌ \u{f035b} ▰▰▰▰▰▰▱▱ 76%     │
│▌ netcup    14/16 ▲  │
│▌ \u{f035b} ▰▰▰▰▰▱▱▱ 65%     │
│▌ momokaya-2  2/7    │
│▌ \u{f035b} ▰▰▰▰▱▱▱▱ 55%     │
╰─────────────────────╯
╭ Agents & Jobs ─── 4 ╮
│▌ web-tests          │
│▌ pi · net…  to land │
│                     │
│▌ i18n-catalog       │
│▌ pi · net…  stalled │
│                     │
│▌ api-dups           │
│▌ pi · mom…  working │
│▌ edit src/api/dups.…│
│                     │
│▌ cargo test --works…│
│▌ job · lo…  running │
╰─────────────────────╯
╭ Todos ───────── 3/7 ╮
│ ━━━━━━━━━━━━ 3 of 7 │
│                     │
│ NOW                 │
│▌ ● Land PR #112     │
│▌   Release it, then │
│▌   update the       │
│▌   installed Herdr  │
│▌   plugin.          │
│                     │
│ NEXT                │
│ ○ Verify the sideba…│
│ ○ Update the dogsle…│
╰─────────────────────╯
 ⏎ focus  tab pane…  «
";

const CAT: Palette = Palette::catppuccin();

fn scenario(name: &str) -> (u16, u16, Model, Palette) {
    let (_, w, h, model, pal) = scenarios().into_iter().find(|s| s.0 == name).unwrap();
    (w, h, model, pal)
}

fn buf(name: &str) -> Buffer {
    let (w, h, model, pal) = scenario(name);
    draw(&model, w, h, &pal)
}

fn scenarios() -> Vec<(&'static str, u16, u16, Model, Palette)> {
    let mut selected = fixture();
    selected.card = 0;
    let mut todos_collapsed = fixture();
    todos_collapsed.toggle_collapse(Panel::Todos);
    let mut expanded = fixture();
    expanded.fold_open = true;
    let mut empty = Model::default();
    empty.data.hosts = hosts();
    let mut stale = fixture();
    stale.data.hosts[1] = HostRow {
        state: HostState::Unreachable,
        memory: None,
        disk: None,
        silent_ms: Some(2 * MIN),
        ..host("netcup", "14/16", HostState::Unreachable, 0.0, 0.0)
    };
    let mut hosts_collapsed = fixture();
    hosts_collapsed.toggle_collapse(Panel::Hosts);
    vec![
        ("full", 36, 37, fixture(), CAT),
        ("selected", 36, 37, selected, CAT),
        ("todos_collapsed", 36, 37, todos_collapsed, CAT),
        ("expanded", 36, 41, expanded, CAT),
        ("empty", 36, 28, empty, CAT),
        ("stale", 36, 37, stale, CAT),
        ("hosts_collapsed", 36, 34, hosts_collapsed, CAT),
        ("auto_collapsed", 36, 19, fixture(), CAT),
        ("w24", 24, 37, fixture(), CAT),
        ("w23", 23, 37, fixture(), CAT),
    ]
}

#[test]
fn the_full_fixture_matches_the_mockup_cell_for_cell() {
    assert_rows(&buf("full"), FULL);
}

#[test]
fn panel_frames_and_titles_are_styled_by_focus() {
    let b = buf("full");
    // Agents & Jobs has focus: accent frame; the others use surface1.
    assert_eq!(style(&b, 0, 0).0, CAT.surface1);
    assert_eq!(style(&b, 0, 8).0, CAT.accent);
    assert_eq!(style(&b, 35, 21).0, CAT.accent);
    assert_eq!(style(&b, 0, 22).0, CAT.surface1);
    let (x, y) = find(&b, "Hosts");
    assert_eq!(style(&b, x, y), (CAT.text, Color::Reset, true));
    let (x, y) = find(&b, " 3 ╮");
    assert_eq!(style(&b, x + 1, y), (CAT.overlay0, Color::Reset, true));
    let (x, y) = find(&b, "3/7");
    assert_eq!(style(&b, x, y), (CAT.overlay0, Color::Reset, true));
    let (x, y) = find(&b, "« ");
    assert_eq!(style(&b, x, y).0, CAT.overlay0);
}

#[test]
fn host_rows_colour_the_bar_and_meter_by_state() {
    let b = buf("full");
    // local and netcup have no headroom (yellow); momokaya-2 is eligible (green).
    assert_eq!(style(&b, 1, 1).0, CAT.yellow);
    assert_eq!(style(&b, 1, 3).0, CAT.yellow);
    assert_eq!(style(&b, 1, 5).0, CAT.green);
    let (x, y) = find(&b, "local");
    assert_eq!(style(&b, x, y), (CAT.text, Color::Reset, true));
    let (x, y) = find(&b, "11/12");
    assert_eq!(style(&b, x, y), (CAT.overlay1, Color::Reset, false));
    let (x, y) = find(&b, "▲");
    assert_eq!(style(&b, x, y).0, CAT.overlay1);
    // 76% of eight cells is six filled, then two empty.
    let (x, y) = find(&b, "▰▰▰▰▰▰▱▱");
    assert_eq!(style(&b, x, y).0, CAT.green);
    assert_eq!(style(&b, x + 5, y).0, CAT.green);
    assert_eq!(style(&b, x + 6, y).0, CAT.surface1);
    let (x, y) = find(&b, "76%");
    assert_eq!(style(&b, x, y).0, CAT.overlay1);
    // The disk meter matches the memory meter: 22% free is 78% in use, six cells filled.
    let disk = find_on(&b, DISK_ICON, y);
    assert_eq!(style(&b, disk, y).0, CAT.overlay1);
    assert_eq!(style(&b, disk + 2, y).0, CAT.green);
    assert_eq!(style(&b, disk + 7, y).0, CAT.green);
    assert_eq!(style(&b, disk + 8, y).0, CAT.surface1);
    let percent = find_on(&b, "78%", y);
    assert_eq!(style(&b, percent, y).0, CAT.overlay1);
}

#[test]
fn the_disk_meter_fills_by_the_share_of_the_disk_in_use() {
    let b = buf("full");
    // local: 22% free is 78% in use. netcup: 24% free is 76% in use.
    let local = find_on(&b, DISK_ICON, 2);
    assert_eq!(style(&b, local + 2, 2).0, CAT.green);
    assert_eq!(style(&b, local + 8, 2).0, CAT.surface1);
    let netcup = find_on(&b, DISK_ICON, 4);
    assert_eq!(style(&b, netcup + 2, 4).0, CAT.green);
    assert_eq!(style(&b, netcup + 8, 4).0, CAT.surface1);
    // Both meters are eight cells wide, so they line up.
    let mem = find_on(&b, MEM_ICON, 2);
    assert_eq!(local - mem, 2 + 8 + 1 + 3 + 2);
    assert_eq!(find_on(&b, "78%", 2) - local, 2 + 8 + 1);
}

#[test]
fn a_narrow_pane_drops_the_disk_percent_and_then_the_disk_meter() {
    // 34 columns: the disk meter keeps its cells, but not the percent.
    let b = draw(&fixture(), 34, 37, &CAT);
    let row = rows(&b)[2].clone();
    assert!(row.contains(&format!("{DISK_ICON} ▰▰▰▰▰▰▱▱")), "{row}");
    assert!(!row.contains("78%"), "{row}");
    assert!(row.contains("76%"), "{row}");
    // 30 columns: the disk meter has no room left, so it goes away.
    let b = draw(&fixture(), 30, 37, &CAT);
    let row = rows(&b)[2].clone();
    assert!(!row.contains(DISK_ICON), "{row}");
    assert!(row.contains(&format!("{MEM_ICON} ▰▰▰▰▰▰▱▱ 76%")), "{row}");
}

#[test]
fn memory_and_disk_turn_yellow_then_red_at_the_admission_limits() {
    let mut model = fixture();
    model.data.hosts[0].memory = Some((0.9, Level::Warn));
    model.data.hosts[1].memory = Some((0.95, Level::Bad));
    model.data.hosts[0].disk = Some((20.0, Level::Warn));
    model.data.hosts[1].disk = Some((10.0, Level::Bad));
    let b = draw(&model, 36, 37, &CAT);
    let (x, y) = find(&b, "▰▰▰▰▰▰▰▱");
    assert_eq!((y, style(&b, x, y).0), (2, CAT.yellow));
    let (x, y) = find(&b, "▰▰▰▰▰▰▰▰");
    assert_eq!((y, style(&b, x, y).0), (4, CAT.red));
    // The disk meter takes the colour of its own level: 20% free is yellow, 10% is red.
    let disk = find_on(&b, DISK_ICON, 2);
    assert_eq!(style(&b, disk + 2, 2).0, CAT.yellow);
    let disk = find_on(&b, DISK_ICON, 4);
    assert_eq!(style(&b, disk + 2, 4).0, CAT.red);
    // The percent stays quiet.
    let (x, y) = find(&b, "95%");
    assert_eq!(style(&b, x, y).0, CAT.overlay1);
    let x = find_on(&b, "80%", 2);
    assert_eq!(style(&b, x, 2).0, CAT.overlay1);
}

#[test]
fn an_unreachable_host_shows_a_question_mark_and_how_long_it_is_silent() {
    let b = buf("stale");
    assert_rows(&b, STALE);
    // The bar is red, the line says `?`, and `no reply 2m` is overlay0.
    assert_eq!(style(&b, 1, 3).0, CAT.red);
    let (x, y) = find(&b, "no reply 2m");
    assert_eq!((y, style(&b, x, y).0), (4, CAT.overlay0));
    let (x, y) = find(&b, "?");
    assert_eq!((y, style(&b, x, y).0), (3, CAT.overlay1));
    // Its cards stay, with dimmed bars and pills; other hosts' cards keep their colours.
    let (_, web) = (0, row_of(&b, "web-tests"));
    assert_eq!(style(&b, 1, web).0, CAT.overlay0);
    let (x, y) = find(&b, "to land");
    assert_eq!(style(&b, x, y).1, CAT.overlay0);
    let api = row_of(&b, "api-dups");
    assert_eq!(style(&b, 1, api).0, CAT.yellow);
    assert!(
        !rows(&b).iter().skip(9).any(|r| r.contains("no reply")),
        "Agents & Jobs has no reply line"
    );
}

#[test]
fn cards_use_state_colours_pills_and_the_selected_background() {
    let b = buf("full");
    let (x, y) = find(&b, "to land");
    assert_eq!(style(&b, x, y), (CAT.crust, CAT.teal, true));
    assert_eq!(
        style(&b, x - 1, y).1,
        CAT.teal,
        "the pill pads one cell each side"
    );
    assert_eq!(style(&b, 1, y).0, CAT.teal);
    let (x, y) = find(&b, "stalled");
    assert_eq!(style(&b, x, y), (CAT.crust, CAT.red, true));
    let (x, y) = find(&b, "working");
    assert_eq!(style(&b, x, y), (CAT.crust, CAT.yellow, true));
    let (x, y) = find(&b, "running");
    assert_eq!(style(&b, x, y), (CAT.crust, CAT.yellow, true));
    let (x, y) = find(&b, "web-tests");
    assert_eq!(style(&b, x, y), (CAT.text, Color::Reset, true));
    let (x, y) = find(&b, "pi · netcup");
    assert_eq!(style(&b, x, y).0, CAT.overlay1);
    let (x, y) = find(&b, "+2");
    assert_eq!(style(&b, x, y), (CAT.green, Color::Reset, true));
    let (x, y) = find(&b, "3m");
    assert_eq!(style(&b, x, y).0, CAT.overlay0);
    // The selected card (api-dups) has surface0 behind all three lines, edge to edge;
    // the pill keeps its own colour.
    let api = row_of(&b, "api-dups");
    for line in api..api + 3 {
        for x in 1..35 {
            if line == api && x >= 26 {
                continue;
            }
            assert_eq!(style(&b, x, line).1, CAT.surface0, "x={x} line={line}");
        }
    }
    assert_eq!(style(&b, 1, api - 2).1, Color::Reset);
    assert_eq!(style(&b, 1, api + 4).1, Color::Reset);
}

#[test]
fn a_selected_first_card_hides_the_activity_of_the_others() {
    let b = buf("selected");
    assert_rows(&b, SELECTED);
    let web = row_of(&b, "web-tests");
    assert_eq!(style(&b, 5, web).1, CAT.surface0);
    assert_eq!(style(&b, 5, web + 1).1, CAT.surface0);
    assert_eq!(style(&b, 5, web + 2).1, Color::Reset);
}

#[test]
fn todos_draw_progress_now_next_and_the_fold() {
    let b = buf("full");
    let bar = row_of(&b, "━");
    assert_eq!(style(&b, 2, bar).0, CAT.green);
    assert_eq!(style(&b, 10, bar).0, CAT.green);
    assert_eq!(style(&b, 11, bar).0, CAT.surface1);
    assert_eq!(style(&b, 22, bar).0, CAT.surface1);
    let (x, y) = find(&b, "3 of 7");
    assert_eq!((x, style(&b, x, y).0), (28, CAT.overlay0));
    let (x, y) = find(&b, "NOW");
    assert_eq!(style(&b, x, y), (CAT.overlay0, Color::Reset, true));
    let (x, y) = find(&b, "Land PR");
    assert_eq!(style(&b, x, y), (CAT.text, CAT.surface0, true));
    assert_eq!(style(&b, 1, y).0, CAT.accent);
    assert_eq!(style(&b, 3, y).0, CAT.accent);
    assert_eq!(
        style(&b, 20, y).1,
        CAT.surface0,
        "NOW rows are surface0 edge to edge"
    );
    let (x, y) = find(&b, "Release it");
    assert_eq!(style(&b, x, y), (CAT.overlay1, CAT.surface0, false));
    let (x, y) = find(&b, "Verify");
    assert_eq!(style(&b, x, y), (CAT.subtext0, Color::Reset, false));
    assert_eq!(style(&b, x - 2, y).0, CAT.subtext0);
    let (x, y) = find(&b, "3 done");
    assert_eq!(style(&b, x, y).0, CAT.green);
    assert_eq!(style(&b, x - 2, y).0, CAT.green);
    let (x, y) = find(&b, "1 dropped");
    assert_eq!(style(&b, x, y).0, CAT.overlay0);
    let (x, y) = find(&b, "▸");
    assert_eq!(style(&b, x, y).0, CAT.overlay0);
}

#[test]
fn collapsing_todos_leaves_its_border_line() {
    let b = buf("todos_collapsed");
    assert_rows(&b, TODOS_COLLAPSED);
    let (_, y) = find(&b, "Todos");
    assert_eq!(style(&b, 0, y).0, CAT.surface1);
}

#[test]
fn the_done_and_dropped_lists_expand_in_place() {
    let b = buf("expanded");
    assert_rows(&b, EXPANDED);
    let (x, y) = find(&b, "Dock the board");
    assert_eq!(style(&b, x - 2, y).0, CAT.green);
    let (x, y) = find(&b, "Plugin sidebar API");
    assert_eq!(style(&b, x - 2, y).0, CAT.overlay0);
    let (x, y) = find(&b, "▾");
    assert_eq!(style(&b, x, y).0, CAT.overlay0);
}

#[test]
fn empty_panels_say_so_in_overlay0() {
    let b = buf("empty");
    assert_rows(&b, EMPTY);
    let (x, y) = find(&b, "No agents or jobs");
    assert_eq!(style(&b, x, y).0, CAT.overlay0);
    assert_eq!(x, 10, "centred in 34 inner columns");
    let (x, y) = find(&b, "No todos for this session");
    assert_eq!((x, style(&b, x, y).0), (6, CAT.overlay0));
}

#[test]
fn hosts_fold_to_one_line_each_with_a_state_bar() {
    let b = buf("hosts_collapsed");
    assert_rows(&b, HOSTS_COLLAPSED);
    assert_eq!(style(&b, 1, 1).0, CAT.yellow);
    assert_eq!(style(&b, 1, 3).0, CAT.green);
    let (x, y) = find(&b, "netcup 14/16");
    assert_eq!(style(&b, x, y), (CAT.text, Color::Reset, true));
}

#[test]
fn a_short_pane_folds_hosts_by_itself() {
    let b = buf("auto_collapsed");
    assert_rows(&b, AUTO_COLLAPSED);
    // A tall pane keeps the two-line rows; the user's own setting is untouched.
    let (_, _, model, _) = scenario("auto_collapsed");
    assert!(!model.hosts_compact);
    assert!(
        !rows(&draw(&model, 36, 37, &CAT))
            .iter()
            .any(|r| r.contains("local 11/12"))
    );
}

#[test]
fn at_24_columns_the_pill_stays_on_line_one() {
    let b = buf("w24");
    assert_rows(&b, W24);
    let (x, y) = find(&b, "to land");
    assert_eq!((x, y, style(&b, x, y).1), (15, 9, CAT.teal));
}

#[test]
fn below_24_columns_the_pill_moves_to_line_two() {
    let b = buf("w23");
    assert_rows(&b, W23);
    let (x, y) = find(&b, "to land");
    assert_eq!(y, row_of(&b, "web-tests") + 1);
    assert_eq!(style(&b, x, y), (CAT.crust, CAT.teal, true));
}

#[test]
fn the_latte_palette_recolours_every_element() {
    let latte = Palette::catppuccin_latte();
    let (width, height, model, _) = scenario("full");
    let b = draw(&model, width, height, &latte);
    assert_rows(&b, FULL);
    assert_eq!(style(&b, 0, 8).0, latte.accent);
    assert_eq!(style(&b, 0, 0).0, latte.surface1);
    let (x, y) = find(&b, "to land");
    assert_eq!(style(&b, x, y), (latte.crust, latte.teal, true));
    let api = row_of(&b, "api-dups");
    assert_eq!(style(&b, 5, api).1, latte.surface0);
    let (x, y) = find(&b, "Land PR");
    assert_eq!(style(&b, x, y), (latte.text, latte.surface0, true));
    let (x, y) = find(&b, "▰▰▰▰▰▰▱▱");
    assert_eq!(style(&b, x, y).0, latte.green);
    assert_eq!(style(&b, x + 6, y).0, latte.surface1);
    assert_eq!(style(&b, 1, 5).0, latte.green);
    assert_ne!(latte.text, CAT.text);
}

#[test]
fn a_long_todo_expands_to_its_full_text_and_children() {
    let mut model = fixture();
    model.focus = Panel::Todos;
    let mut data = todo_data();
    data.sections[1].items[1]
        .children
        .push(crate::board::TodoChild {
            status: TodoStatus::Completed,
            text: "Write the docs".into(),
            description: None,
            unit: None,
        });
    model.data.todos = Some(data);
    model.todo = 2;
    model.open_todo(true);
    let b = draw(&model, 36, 41, &CAT);
    let text = rows(&b).join("\n");
    assert!(text.contains("○ Update the dogsled skill and"), "{text}");
    assert!(text.contains("Rust docs"), "{text}");
    assert!(text.contains("✓ Write the docs"), "{text}");
    // The focused panel takes the accent frame, and the selected row its surface0 background.
    assert_eq!(style(&b, 0, 22).0, CAT.accent);
    let (x, y) = find(&b, "Update the dogsled");
    assert_eq!(style(&b, x, y).1, CAT.surface0);
}

#[test]
fn scrolling_keeps_the_selection_in_view_and_the_wheel_moves_the_viewport() {
    let mut model = fixture();
    model.card = 3;
    let area = Rect::new(0, 0, 36, 25);
    let geo = geometry(&model, area, &CAT);
    let range = geo.agents.items[3].clone();
    let rows_shown = usize::from(geo.agents.body.height);
    assert!(
        geo.agents.scroll + rows_shown >= range.end,
        "the last card is in view"
    );
    assert!(geo.agents.lines.len() > rows_shown, "the panel does scroll");
    settle(&mut model, &geo);
    model.scroll_by(Panel::Agents, -2);
    let geo = geometry(&model, area, &CAT);
    assert_eq!(geo.agents.scroll, model.scroll[0]);
    assert!(model.scroll[0] + 2 > 0 && !model.follow);
    // Hosts never scrolls.
    assert_eq!(geo.hosts.scroll, 0);
}

#[test]
fn agents_take_at_most_sixty_percent_when_both_panels_do_not_fit() {
    let model = fixture();
    let area = Rect::new(0, 0, 36, 31);
    let geo = geometry(&model, area, &CAT);
    // 30 rows below the footer: Hosts is folded (22 left < 12 is false, so it stays full at 8).
    assert_eq!(geo.hosts.rect.height, 8);
    let rest = 30 - 8;
    assert!(geo.agents.rect.height <= (f32::from(rest) * 0.6).floor() as u16 + 1);
    assert_eq!(geo.agents.rect.height + geo.todos.rect.height, rest);
    assert!(geo.todos.rect.height >= 3);
}

#[test]
fn hit_testing_reports_what_was_drawn() {
    let model = fixture();
    let geo = geometry(&model, Rect::new(0, 0, 36, 37), &CAT);
    assert_eq!(geo.hit(34, 36), Some(Hit::Close));
    assert_eq!(geo.hit(5, 0), Some(Hit::Frame(Panel::Hosts)));
    assert_eq!(geo.hit(5, 2), Some(Hit::Empty(Panel::Hosts)));
    assert_eq!(geo.hit(5, 9), Some(Hit::Card(0)));
    assert_eq!(
        geo.hit(5, 11),
        Some(Hit::Empty(Panel::Agents)),
        "the gap between cards"
    );
    assert_eq!(geo.hit(5, 12), Some(Hit::Card(1)));
    assert_eq!(geo.hit(5, 26), Some(Hit::Todo(0)));
    assert_eq!(geo.hit(5, 34), Some(Hit::Todo(3)));
}

#[test]
fn text_helpers_clip_and_wrap_by_cells() {
    assert_eq!(fit("abcdef", 6), "abcdef");
    assert_eq!(fit("abcdef", 5), "abcd…");
    assert_eq!(fit("界界界", 5), "界界…");
    assert_eq!(fit("abc", 0), "");
    assert_eq!(wrap("one two three", 7), ["one two", "three"]);
    assert_eq!(wrap("abcdefgh", 3), ["abc", "def", "gh"]);
    assert_eq!(wrap("", 4), [""]);
}
