# Sidebar dock keeps the caller's focus

**Outcome.** After `board --open` (or `--toggle` opening the dock), the pane that had Herdr focus before the call has focus again, so a Pi pane that starts the sidebar keeps the keyboard.

**Cause.** `dock` in `src/board.rs` (lines 267-361) opens the plugin pane with `--no-focus`, then runs `herdr pane swap --source-pane <new sidebar> --target-pane <leftmost>` (lines 337-349). In Herdr 0.9.5, an explicit source/target swap always focuses the source pane and switches to its tab (`handle_pane_swap`, herdr `src/app/api/panes.rs`: `tab.layout.focus_pane(source_pane_id)` then `switch_workspace_tab`). The source is the sidebar, so the sidebar takes focus. Swapping source and target is not a fix: focus would then go to whatever pane held the leftmost slot, which is not always the caller.

**Design.**
- The `herdr pane list` reply that `dock` already parses (line 280) has a boolean `focused` on each pane. Exactly one pane is `true` (global focus). Take that pane's `pane_id` before the open/swap; fall back to `owner` when none is `true`.
- After `set_split_ratio` (line 356), restore focus with the Herdr socket method `pane.focus`, params `{"pane_id": "<id>"}` (herdr `PaneTarget { pane_id }`; reply `result.type == "pane_info"`). `pane.focus` also switches to that pane's tab, which undoes the swap's tab switch.
- Add `pub fn focus_pane(&self, pane_id: &str) -> Result<Value>` to `Client` in `src/herdr_socket.rs` (next to `set_split_ratio`, lines 92-101). Use `self.call("pane.focus", &json!({"pane_id": pane_id}))`; do not route it through `layout()`, since the reply has no `layout`.
- Only the opening path restores focus. The reuse path (`live && !toggling`) and the toggle-close path run no swap and must not call `pane.focus`.
- A `pane.focus` error is an error from `dock`, like the other socket calls. No fallback.

**Edits.**
1. `src/herdr_socket.rs`: `focus_pane` as above.
2. `src/board.rs` `dock`: capture the focused pane id from `panes`; call `socket.focus_pane(&focused)` after `set_split_ratio`, before `registry.write`.
3. `src/board.rs` tests (`dock_rig`, ~line 983): give the fake `pane list` panes a `focused` field (owner `w1:p1` true by default), answer `"pane.focus"` in the fake socket with `{"result":{"type":"pane_info","pane":{}}}`. `ratio_request` asserts the *last* request is `set_split_ratio`; change it to find the `layout.set_split_ratio` request instead.
4. New tests in the same module:
   - opening the dock ends with a `pane.focus` request for `w1:p1` (the last socket request);
   - when another pane in the list is the focused one (add e.g. `w1:p9` with `focused: true`, owner false), `pane.focus` targets that pane;
   - reusing a live dock and toggle-closing send no `pane.focus` request.
5. `CHANGELOG.md` under `## Unreleased`: one Fixed line: opening the sidebar no longer takes focus from the pane that opened it.

**Owned paths.** `src/board.rs`, `src/herdr_socket.rs`, `CHANGELOG.md`.

**Acceptance.** `cargo test --locked board` passes, including the new tests.

**Checks.** `cargo test --locked board`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`. The lefthook pre-commit hook runs; do not bypass it.

**Base.** `origin/main` at `4e2be386b8926a40afd4e585918a15339cf6151d`. Branch `fix/sidebar-keep-focus`. Commit, push, open a draft PR. No merge, release, or install.
