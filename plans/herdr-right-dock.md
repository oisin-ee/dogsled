# Plan: a real Herdr right dock for dogsled

Status: skeleton, 2026-10-22. Open questions at the end.

## Context

- The user asked for a narrow, full-height right panel that looks and works like Herdr's own sidebar, and shows quota and work clearly.
- PR #92 (`29f38d6`) shipped something else. `src/board.rs` `show_side` (origin/main 5d4bd18) prints plain text with `\x1b[2J` every 10 s (`:410`, `:423`). `open` (`:140-195`) splits the owner pane 50/50 to the right (`--placement split --direction right --target-pane <owner>`) and never resizes.
- Defects visible in the user's screenshot: uppercase headers, two text lines per quota window, no bars or reset times, internal unit IDs, and the owner's todo printed twice (`render_side` `:299-320`).

### Facts from research (Herdr 0.9.5; preview fbd2ae2 docs are identical)

- Plugins can't draw native non-terminal UI: "native non-terminal plugin UI [is] not part of plugin v1" (plugins.mdx). A dock must be a terminal pane.
- `plugin pane open` takes no ratio. A follow-up `pane resize` (fraction) sets the width. There is no root split, so a stacked rightmost column gives only the top slot.
- Native slots a plugin can fill:
  - Pane tokens via `pane report-metadata`, shown as `$name` in agent rows.
  - `agent.view.set` to filter and sort the native agents panel.
  - `ui.tab_bar_right` command entries.
- `events.subscribe` gives push updates for agent and pane changes.
- Herdr's sidebar look (`src/client/shell/{sidebar,agent_sidebar}.rs`, `shell.rs:178-237`):
  - Lowercase headers in `overlay0` bold, with a `─` divider in `surface_dim`.
  - Rows indented 1, then 3. Names in `subtext0` bold, details in `overlay0`, joined with ` · `.
  - Dots: `●` working/blocked/done, `○` idle, `·` unknown. Colours: yellow, red, teal, green.
- Prior art:
  - alexarthurs/herdr-sidebar 081ed13: ratatui dock, 32 columns by default, re-docks on tab and workspace events.
  - thanhdat77/herdr-navigator 313b753: open/focus/close toggle, and theme read from `[theme]` in the Herdr config.

## Approach

1. **Dock pane:** a ratatui TUI inside the dogsled binary (`dogsled board --dock`), replacing the `print!` loop.
2. **Placement:** split the tab's rightmost pane, not the owner pane. Then resize to the target width (default 32 columns) with `pane resize`. Keep one dock per tab, as `board::open` already does.
3. **Look:** Herdr's sidebar style, using the palette from the user's Herdr `[theme]`/`[theme.custom]`.
4. **Sections:**
   - quota: one line per window, with a bar, percent and reset countdown from `resets_ms`;
   - work: grouped rows from `board::build`;
   - todo: the owner's structured items.
   No unit IDs and no raw provider lines.
5. **Refresh:** on ledger and Herdr events, not a 10 s clear-screen.
6. **Input:** Enter or a click on a work row focuses that agent's pane. `q` closes the dock.
7. **Native slots:** keep `$dogsled` and `status --line`, and add `$todo` for each child pane.

## Files to modify (draft)

- `src/board.rs`: replace `show_side`/`render_side` with a view model; fix the placement in `open`.
- `src/board/dock.rs` (new): ratatui rendering and input.
- `src/cli.rs`: the `board --dock` flag, replacing `--side --watch`.
- `herdr-plugin.toml`: pane command, toggle action, key snippet in the README.
- `Cargo.toml`: `ratatui` 0.30, `crossterm` 0.29.
- `pi/index.ts`: open path only (see Q3).
- `README.md`, `CHANGELOG.md`, tests in `src/board.rs`.

## Reuse

- `board::build`, `Board.rows`, `Group` (`src/board.rs`): the work data.
- `quota::cached`, `quota::Row.resets_ms` (`src/quota.rs`): the quota data.
- `board::cached_todos`, `TodoLines` (`src/board.rs:72-119`): the todo data.
- `board::open` registry and lock (`src/board.rs:140-195`): one dock per tab.
- `watchdog`/`jobs` `report-metadata` calls (`src/watchdog.rs:160,201`, `src/jobs.rs:150`): the token pattern.

## Steps

- [ ] Mockup in the user's catppuccin palette, for approval before code.
- [ ] View model and pure render tests (ratatui `TestBackend`).
- [ ] Placement: rightmost pane, then resize, then the per-tab registry.
- [ ] Event-driven refresh and input.
- [ ] Manifest, toggle action, README.
- [ ] Research note in the PR body (new dependency, external API).

## Verification

- `cargo test` for render snapshots at widths 24, 32 and 48.
- Manual run in Herdr 0.9.5: open from Pi, check it docks at the right edge at about 32 columns, check it updates when a child changes state, and check Enter focuses the child.

## Open questions

See the chat message for this revision.
