# Plan: dogsled sidebar, design C (framed panels)

Status: draft for review, 2026-10-22. Design C picked from three rendered mockups.

## Context

- The user wants a second sidebar docked against Herdr's own left sidebar, like alexarthurs/herdr-sidebar, with real sections: **Hosts** (top, added 2026-10-22 at the user's request), **Agents & Jobs**, and **Todos**. No quotas.
- The user picked **design C: framed panels**. Each section is its own rounded, bordered panel with a title and count. Agents and jobs are cards with a state bar and state pill. Todos have a progress bar, **NOW**, **NEXT**, and one folded done/dropped line.
- The shipped `board --side` (`src/board.rs` `render_side`/`show_side`, origin/main 5d4bd18) is a plain-text dump in a 50/50 split. It is replaced, not restyled.
- Herdr 0.9.5 has no plugin sidebar API, and its own sidebar is fixed at column 0 (`src/client/shell/config.rs:442`). herdr-sidebar uses a ratatui TUI in a plugin pane, and dogsled does the same.

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

### Card order and states

- Cards follow `board::build` groups: To land, Needs you (attention), Running, Waiting. Inside a group, the newest is first.
- Pills: `to land`, `stalled`, `blocked`, `working`, `idle`, `running` (job), `queued`, `failed` (job, red).
- Empty states: `No agents or jobs` / `No todos for this session` in `overlay0`, centred in the panel.
- A remote host that does not answer shows `no reply 2m` in the Hosts panel. Its last known cards stay, marked stale (dim border).

### Panel heights

- Hosts takes its fixed height first (2 lines per host + borders; 1 line per host when collapsed). Agents & Jobs and Todos share the rest: each sizes to its content, and when both do not fit, Agents & Jobs gets at most 60% of the remaining height and scrolls inside its frame, and Todos takes the rest. When a panel is collapsed (`space`), it shrinks to its border line and the other panels grow.
- When the pane is too short for Hosts expanded plus 3 lines for each other panel, Hosts renders collapsed.
- Below 24 columns, the pill moves to line 2 and the name gets the full line.

### Interaction

- `tab` moves focus Hosts -> Agents & Jobs -> Todos; the focused border is `accent`. Host rows are selectable but have no actions in this version.
- `↑↓`/`jk` move between cards or todo rows. `⏎` focuses the agent's or job's Herdr pane (`herdr agent focus` / `herdr pane focus`).
- On a todo row, `⏎` or `→` shows the full text and subtasks in place. On the done/dropped line, it expands those lists in place.
- `m` opens an inline menu on the selected card: Focus, Stop job (`work::stop`), Cancel queued (`work::cancel`), Close finished child (`close::local`). Destructive items ask `y/n` inline.
- Mouse: click selects, double-click focuses, wheel scrolls the panel under the cursor, click `«` closes the sidebar.
- Fold and focus state persist per owner in `sidebar/<owner>.json` under the ledger.

## Approach

### 1. Todo data from pi-todo as typed sections (oisinpi)

- pi-todo's core already groups tasks by status (`~/dev/oisinpi/crates/todo-core/src/widget.rs` `group`/`widget`). It flattens them into strings like `"In progress · 1"`, which design C cannot lay out.
- Add `sections(list) -> Vec<TodoSection>`: `{status, count, items: [{status, text, review, unit, children}]}`. Rebuild `widget()` on top of it so one function owns grouping.
- pi-todo publishes it as `display.sections`. This is protocol version 3 (`docs/pi-work.md`, pi-todo `src/work.ts`). pi-todo still owns all task interpretation; dogsled only draws it.

### 2. dogsled publishes sections, not rendered lines

- `pi/work-todo.ts` `TodoPublisher` writes `{session, branch, revision, pane, sections}` to the existing ledger path (`board --todo-path`). `renderTodoLines` and the width/height handshake are removed.
- `src/board.rs` `TodoLines` becomes `TodoSections`. `collect_todos`, `cached_todos`, and the remote `todos` call keep their paths and validation.

### 3. The sidebar app (`dogsled sidebar`, ratatui)

- New module `src/sidebar/`:
  - `model.rs`: cards, todo sections, selection, fold, and scroll state.
  - `view.rs`: the three panels (Hosts, Agents & Jobs, Todos) with `Block::bordered().border_type(Rounded)`, cards, pills, progress bar, footer.
  - `input.rs`: keys and mouse.
  - `palette.rs`: the Herdr theme.
- A reader thread rebuilds the model from cached ledger state every 2 s, and from remote todos every 10 s. Input never waits for a read. ratatui only redraws changed cells.
- Dependencies: ratatui 0.30.2 (`crossterm_0_29`, no defaults) and crossterm 0.29.0, already on branch `chore/tui-deps` (96e5dcd). Herdr 0.9.5 uses the same versions.

### 4. Docking against Herdr's sidebar

1. Use `herdr pane layout` to find the leftmost top-level pane of the lead's tab.
2. Run `herdr plugin pane open --entrypoint sidebar --placement split --direction right --target-pane <leftmost> --no-focus`.
3. `herdr pane swap` moves the new pane into the left slot.
4. Call `layout.set_split_ratio` so the pane is 38 cells (36 inner). This method is socket-only, so a small client in `src/herdr_socket.rs` sends it.

- There is one sidebar per lead tab, from the existing `board::open` registry. A Dogsled-launched child never opens one (`launch::launched`).
- When the leftmost column is split top/bottom, the sidebar takes the top slot. It does not move other panes.
- Manifest: `[[panes]] id = "sidebar"`, `title = "dogsled"`, command `["bin/dogsled", "sidebar"]`; `[[actions]] id = "toggle"` for a key binding.
- `pi/index.ts` `openSide` keeps opening it at lead session start and hides the inline widget while it is open.

### Removed

`render_side`, `show_side`, `quota_lines`, `pane_width`, `compact`, `board-widths`, `--side`/`--watch`, the `TodoLines` renderer, and the `[[panes]] board` entry. The quota collector and `status --line` stay, but the sidebar does not show them.

## Files to modify

- oisinpi: `crates/todo-core/src/widget.rs` (+ `sections`), `crates/todo-wasm/src/lib.rs`, `packages/pi-todo/src/{display,work}.ts`, generated wasm, changeset.
- dogsled:
  - New: `src/sidebar/{mod,model,view,input,palette}.rs`, `src/herdr_socket.rs`.
  - Changed: `src/board.rs`, `src/cli.rs`, `src/main.rs`, `pi/work-todo.ts`, `pi/index.ts`, `herdr-plugin.toml`, `docs/pi-work.md`, `README.md`, `CHANGELOG.md`, tests.
  - `Cargo.toml` and `Cargo.lock` come from 96e5dcd.

## Reuse

- `board::build`, `Group`, `Row` (`src/board.rs`) for the cards.
- The `board::open` registry and `launch::launched`, for one sidebar per tab.
- `collect_todos`, `cached_todos`, `Fleet::remote("todos")` for todo transport.
- `work::stop`, `work::cancel`, `close::local` for menu actions.
- pi-todo `group()`, moved into `sections()`.
- herdr-sidebar's docking order (`ensure.rs:740-840`), read only, nothing copied.

## Steps

- [ ] Unit 1 (oisinpi): `sections()` + `display.sections`, protocol v3, release.
- [ ] Unit 2 (dogsled, parallel with 1): left dock, exact width, toggle action, socket client.
- [ ] Unit 3 (dogsled, after 1): sidebar model, view, input, and palette; publisher change; removals.

## Verification

- Unit 1: `cargo test -p todo-core` and the pi-todo tests.
- Unit 2: `cargo test` for `board_open` with a fake Herdr shell and socket that asserts the call sequence and the 38-cell ratio.
- Unit 3: ratatui `TestBackend` buffer tests that match the mockup above, cell for cell. Cover:
  - the full fixture;
  - a selected card;
  - Todos collapsed;
  - done/dropped expanded;
  - empty panels;
  - a stale remote host (Hosts `?`/`no reply`, dim cards);
  - the Hosts panel: eligible, `▲`, and unreachable rows; memory and disk colour thresholds; collapsed; auto-collapse on a short pane;
  - a 24-column width;
  - the `catppuccin-latte` palette.
- The dogsled gate runs once at the final revision.
- Manual check: link the plugin in a worktree, then start a Pi lead in Herdr with one child and one job. The dock should open on the left beside Herdr's sidebar, and the panels should match the mockup. `⏎` should focus the child. Todos should update after a `todo` call. I send you a screenshot before merge.
