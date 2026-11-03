# Unit 3 — the dogsled sidebar app (design C: framed panels)

Unit: u-9fbe63a3-6003-4d0f-b39d-8cb412c11b91-12. Replace the text side board with `dogsled sidebar`, a ratatui app in the docked plugin pane, drawing three framed panels — **Hosts** (top), **Agents & Jobs**, and **Todos** — exactly as the approved mockup below. The Hosts panel was added after dispatch; its spec is the `### Hosts panel` section and the host rows of the mockup and style table in `plans/dogsled-sidebar.md` (copied at the end of this brief). Todos come from pi-todo's typed `display.sections` (protocol v3).

Repository and branch: oisin-ee/dogsled, branch `feat/sidebar-app`.
Base SHA: 6248dbeeab8efa8f0d04801917ce4a23111eb72f (origin/main, unit 2 merged as PR #119).
Prerequisites: unit 2 merged — `board::open`/`board::toggle`/`dock` in `src/board.rs` (left dock, swap, `set_split_ratio`), `src/herdr_socket.rs` (`Client`, `fake::Server` for tests), `[[actions]] toggle` in `herdr-plugin.toml`. These hard-code entrypoint `board`; rename it to `sidebar` in the same commit as the manifest change. Herdr clamps split ratios to 0.1–0.9 (`src/layout.rs:295`). Unit 1 (oisinpi `feat/todo-sections`) defines the todo contract below; it may still be in review — build against the contract, not the package.

## Approved design (36 inner columns; Herdr draws the outer border and the `dogsled` title)

```
╭ Agents & Jobs ──────────────── 6 ╮   panel 1: title left, count right
│▌ web-tests               to land │   card line 1: state bar, name, state pill
│▌ pi · netcup               +2 3m │   card line 2: harness · host, +commits, age
│                                  │
│▌ i18n-catalog            stalled │
│▌ pi · netcup                 22m │
│                                  │
│▌ api-dups                working │   selected card: surface0 background
│▌ pi · momokaya-2             14m │
│▌ edit src/api/dups.rs            │   selected card adds line 3: activity
│                                  │
│▌ cargo test --workspace  running │   jobs are cards too: "job · host"
│▌ job · local                  4m │
╰──────────────────────────────────╯
╭ Todos ────────────────────── 3/7 ╮   panel 2: count = completed/total
│ ━━━━━━━━━━━━━━━━━━━━━     3 of 7 │   progress bar: green done, surface1 rest
│                                  │
│ NOW                              │
│▌ ● Land PR #112                  │   in_progress: headline (text), bold, one line
│▌   Release it, then update the   │   description underneath, wrapped, overlay1
│▌   installed Herdr plugin.       │
│                                  │
│ NEXT                             │
│ ○ Verify the sidebar on netcup   │   pending items, one line each, … truncation
│ ○ Update the dogsled skill and R…│
│                                  │
│ ✓ 3 done   − 1 dropped         ▸ │   folded; ⏎/→ expands the lists in place
╰──────────────────────────────────╯
 ⏎ focus  tab panel  m menu       «    footer: key hints, « closes
```

Style — Herdr 0.9.5 palette (`herdr` source `src/app/state.rs:78`, catppuccin: accent 137,180,250; surface0 49,50,68; surface1 69,71,90; overlay0 108,112,134; overlay1 127,132,156; text 205,214,244; subtext0 166,173,200; green 166,227,161; yellow 249,226,175; red 243,139,168; teal 148,226,213; crust 17,17,27; latte at :103):

| Element | Style |
|---|---|
| Panel border | `BorderType::Rounded`, `surface1`; `accent` when the panel has focus |
| Panel title | `text` bold; count right-aligned in `overlay0` bold |
| State bar `▌` and pill | to land `teal`, stalled/blocked/failed `red`, working/running `yellow`, idle `green`, queued `overlay0`; pill text `crust` bold on the state colour, padded ` x ` |
| Card line 2 | `overlay1`; `+N` commits `green` bold; age `overlay0` |
| Selected card | `surface0` background on all its lines |
| NOW rows | `accent` `▌` and `●`; headline `text` bold, truncated to one line; description `overlay1`, wrapped, indented under the headline; `surface0` background. No description: the headline wraps instead |
| NEXT rows | `○` and text in `subtext0` |
| Done/dropped line | `✓ N done` `green`, `− N dropped` `overlay0`, `▸`/`▾` `overlay0` |
| Footer | `overlay0` |

Palette source: Herdr's `[theme]` in `$XDG_CONFIG_HOME/herdr/config.toml` (default `~/.config/herdr/config.toml`): `name` `catppuccin` / `catppuccin-latte` built-ins, then `[theme.custom]` overrides, in Herdr's order (see herdr `src/config/theme.rs`). Any other theme name → Herdr's `terminal` ANSI palette. Parse with the existing `toml` dependency. No other new crates.

Cards: from `board::build` (`src/board.rs:715`) groups To land, Attention, Running, Waiting; newest first within a group. Pills: `to land`, `stalled`, `blocked`, `working`, `idle`, `running` (job), `queued`, `failed` (job). Empty panels: `No agents or jobs` / `No todos for this session`, `overlay0`, centred. An unreachable remote host: `?` and `no reply 2m` in the Hosts panel; its last cards stay with a dimmed (`overlay0`) border.

Heights: Hosts takes its fixed height first (auto-collapses on a short pane); the other two panels size to content; when both do not fit, Agents & Jobs gets at most 60% and scrolls inside its frame, Todos takes the rest. `space` collapses the focused panel to its border line and the other grows. Below 24 columns the pill moves to line 2.

Interaction: `tab` cycles Hosts -> Agents & Jobs -> Todos. `↑↓`/`jk` move. `⏎` focuses the card's Herdr pane (`herdr agent focus` / `herdr pane focus`). On a todo row `⏎`/`→` shows full text + children in place; on the done/dropped line it expands those lists. `m` opens an inline menu on the selected card: Focus, Stop job (`work::stop`), Cancel queued (`work::cancel`), Close finished child (`close::local`); destructive items confirm `y/n` inline. Mouse: click selects, double-click focuses, wheel scrolls the panel under the cursor, click `«` closes the dock. Fold/focus state persists in the ledger at `sidebar/<owner>.json`.

Refresh: a reader thread rebuilds the model from cached ledger state every 2 s and remote todos every 10 s; input never waits on a read. Use crossterm's event stream/poll, alternate screen, mouse capture; restore the terminal on panic and exit.

## Todo contract (protocol v3, from unit 1)

`display.sections`: always four entries in order `in_progress`, `pending`, `completed`, `abandoned`:
`{ status, count, items: [{ status, text, description?: string, review: bool, unit: string|null, children: [{ status, text, description?: string, unit: string|null }] }] }`. `text` is the headline; `description` (unit 1b, optional, absent on old todos) is the detail shown under NOW headlines and in an expanded todo row. NEXT rows show headlines only.

- `pi/work-todo.ts`: accept version `3` (reject others as today); `TodoPublisher` writes `{session, branch, revision, current, pane, sections}` to the existing `board --todo-path` file. Remove `renderTodoLines` and the width/height handshake. Keep the preflight/claim lifecycle in `WorkTodo` (the inline widget still uses `createView`).
- `src/board.rs`: `TodoLines` → `TodoSections` with serde validation of that shape; `collect_todos`, `cached_todos`, remote `todos` keep their paths.
- `docs/pi-work.md`: v3 event table and the sidebar section.

## Code layout

New `src/sidebar/{mod,model,view,input,palette}.rs`; `dogsled sidebar` subcommand in `src/cli.rs`/`src/main.rs`. `herdr-plugin.toml`: rename pane id `board` → `sidebar`, command `["bin/dogsled", "sidebar"]`, update `board --open`/`--toggle` to the new entrypoint (or move them to `sidebar --open/--toggle`, keeping one owner of the registry). `pi/index.ts` `openSide` calls the new command.
Dependencies (add exactly, they match Herdr 0.9.5): `crossterm = "0.29.0"`, `ratatui = { version = "0.30.2", default-features = false, features = ["crossterm_0_29", "layout-cache", "underline-color"] }`; commit the updated `Cargo.lock`.
Remove: `render_side`, `show_side`, `quota_lines`, `pane_width`, `compact`, `board-widths`, `--side`/`--watch`, the `TodoLines` renderer. The quota collector and `status --line` stay.

## Scope

Owned paths: `src/sidebar/`, `src/board.rs`, `src/cli.rs`, `src/main.rs`, `pi/work-todo.ts`, `pi/index.ts`, `pi/*.test.ts` touched by these, `herdr-plugin.toml`, `docs/pi-work.md`, `README.md`, `CHANGELOG.md`, `Cargo.toml`, `Cargo.lock`, `tests/`.
Generated paths: none.
Read first: `src/board.rs` (build, Group, Row, todo transport, open/toggle from unit 2), `src/work.rs` (stop/cancel), `src/close.rs`, `pi/work-todo.ts`, `pi/index.ts`, `docs/pi-work.md`, ratatui `TestBackend` docs https://docs.rs/ratatui/0.30.2/ratatui/backend/struct.TestBackend.html .

## Checks

Acceptance (you run it): `cargo test --locked sidebar && npm test` — passes, including ratatui `TestBackend` buffer tests asserting the layout above cell for cell (symbols and key styles) for: the full fixture; a selected card; Todos collapsed; done/dropped expanded; both panels empty; a stale remote host; Hosts eligible/`▲`/unreachable rows, memory and disk colour thresholds, Hosts collapsed and auto-collapsed; 24-column width; the `catppuccin-latte` palette. Plus model tests for selection/fold/scroll, input tests for keys and mouse hits, and Pi shim tests for the v3 publisher.

Allowed light checks: `cargo test --locked sidebar`, `cargo test --locked board`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`, `npm run typecheck`, `npm test`. The lefthook pre-commit hook runs `mise run check:rust` / `check:pi`; do not bypass it.
Forbidden heavy checks: `mise run check`, release builds, `herdr plugin install`, anything against the user's live Herdr session.

## Publication authority

Commit: yes. Push `feat/sidebar-app`: yes. Draft PR (with a short research note: Herdr has no plugin sidebar API, docking approach, palette source): yes. Merge, release, tag, install: no.

Failure behaviour: if the design cannot be met as drawn at some width, stop and report with a rendered `TestBackend` dump; do not redesign it.

## Reply

Branch, head SHA, draft PR URL, changed paths, commands with results, a plain-text dump of the full-fixture `TestBackend` buffer, and unverified scope.


## Hosts panel addendum (from plans/dogsled-sidebar.md, authoritative)

## Design C (approved mockup, 36 inner columns)

```
┌ dogsled ───────────────────────────┐   Herdr pane border + title
│╭ Hosts ──────────────────────── 3 ╮│   panel 1 (top): one 2-line row per host
││▌ local                  11/12 ▲  ││   line 1: state bar, label, busy/cores, marker
││▌ mem ▰▰▰▰▰▰▱▱ 76%  disk 22% free ││   line 2: memory meter, disk free
││▌ netcup                 14/16 ▲  ││
││▌ mem ▰▰▰▰▰▱▱▱ 65%  disk 24% free ││
││▌ momokaya-2               2/7    ││
││▌ mem ▰▰▰▰▱▱▱▱ 55%  disk 39% free ││
│╰──────────────────────────────────╯│
│╭ Agents & Jobs ──────────────── 6 ╮│   panel 2: title left, count right
││▌ web-tests               to land ││   card line 1: state bar, name, state pill
││▌ pi · netcup               +2 3m ││   card line 2: harness · host, commits, age
││                                  ││
││▌ i18n-catalog            stalled ││
││▌ pi · netcup                 22m ││
││                                  ││
││▌ api-dups                working ││   selected card: surface0 background
││▌ pi · momokaya-2             14m ││
││▌ edit src/api/dups.rs            ││   selected card adds line 3: activity
││                                  ││
││▌ cargo test --workspace  running ││   jobs are cards too: "job · host"
││▌ job · local                  4m ││
│╰──────────────────────────────────╯│
│╭ Todos ────────────────────── 3/7 ╮│   panel 3
││ ━━━━━━━━━━━━━━━━━━━━━     3 of 7 ││   progress bar: green done, surface1 rest
││                                  ││
││ NOW                              ││
││▌ ● Land PR #112                  ││   in-progress: headline (content), bold, one line
││▌   Release it, then update the   ││   description underneath, wrapped, overlay1
││▌   installed Herdr plugin.       ││
││                                  ││
││ NEXT                             ││
││ ○ Verify the sidebar on netcup   ││   pending tasks, one line each
││ ○ Update the dogsled skill and R…││
││                                  ││
││ ✓ 3 done   − 1 dropped         ▸ ││   folded; ▸/→ expands the list in place
│╰──────────────────────────────────╯│
│ ⏎ focus  tab panel  m menu       « │   footer: key hints, « closes
└────────────────────────────────────┘
```

### Style (Herdr 0.9.5 palette, `src/app/state.rs:78`)

| Element | Style |
|---|---|
| Panel border | rounded (`BorderType::Rounded`), `surface1`; `accent` when the panel has focus, like Herdr pane borders (`src/ui/panes.rs:600`) |
| Panel title | `text` bold; count right-aligned in `overlay0` bold |
| State bar `▌` and pill | to land `teal`, stalled/blocked `red`, working/running `yellow`, idle `green`, queued `overlay0`; pill text `crust` on the state colour |
| Card line 2 | `overlay1`; `+N` commits `green` bold; age `overlay0` |
| Selected card | `surface0` background on all its lines |
| NOW rows | `accent` bar and `●`; headline `text` bold; description `overlay1`, wrapped, indented under the headline; `surface0` background. No description: the headline wraps instead |
| NEXT rows | `○` and text in `subtext0` |
| Done/dropped line | `✓ N done` `green`, `− N dropped` `overlay0` |
| Footer | `overlay0` |
| Host state bar | `green` eligible; `yellow` no headroom (`▲`); `red` unreachable or service down (`?`) |
| Host line 1 | label `text` bold; `busy/cores` and marker `overlay1`, right-aligned |
| Memory meter | 8 cells, `▰` filled / `▱` empty, `green`, then `yellow` within 10% of the admission memory reserve, `red` past it; percent `overlay1` |
| Disk free | `overlay1`; `yellow` below 2x `disk_free_min_percent`, `red` below it |

### Hosts panel

- Data: the same host reports as `dogsled status` (`commands::status` -> `hosts::HostReport`: `probe`, `headroom`, `reachable`, `service`), read from the cached service snapshots. The sidebar never runs its own probe or SSH call.
- Line 1 is the same text as `status --line` for that host: `commands::load` (busy cores over the last interval, `load N/cores` before the first sample) and `commands::marker`. Move both behind a typed accessor that `status` and the sidebar share; do not parse status strings.
- Thresholds for colours come from the host's admission `Thresholds` (`src/config.rs:80`), so red in the panel means admission would refuse.
- Unreachable host: line 1 shows `?`, line 2 shows `no reply 2m` (age of the last snapshot) in `overlay0`. Its cards in Agents & Jobs stay, marked stale (dim border).
- Order: local first, then config order. The title count is the number of hosts.
- Collapsed (`space`): one line per host, `netcup 14/16 ▲`, with the state bar. The panel never scrolls.

The palette is read from Herdr's `[theme]` in `~/.config/herdr/config.toml`: built-in `catppuccin` and `catppuccin-latte`, then `[theme.custom]` overrides, in Herdr's order. Other theme names use Herdr's `terminal` ANSI palette.

