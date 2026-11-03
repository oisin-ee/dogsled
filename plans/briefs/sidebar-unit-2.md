# Unit 2 — dock the dogsled side pane on the left, 38 cells, with a toggle

Unit: u-9fbe63a3-6003-4d0f-b39d-8cb412c11b91-11. `board --open` docks dogsled's plugin pane against Herdr's own left sidebar at exactly 38 cells, instead of a 50/50 split to the right of the lead pane. A Herdr plugin action toggles it.

Repository and branch: oisin-ee/dogsled, branch `feat/sidebar-dock`.
Base SHA: 5d4bd184b7247cc8da1d1281f1a605907270e176 (origin/main, v0.20.0).
Prerequisites: none. Unit 3 (the ratatui sidebar app) builds on this branch after it merges; keep the entrypoint id `board` and its current command so main stays green. Unit 3 renames it.

## Why

The user wants a second sidebar like alexarthurs/herdr-sidebar: a column docked beside Herdr's native sidebar (which is fixed at column 0 in Herdr 0.9.5). Herdr has no plugin sidebar API, so the column is a plugin pane placed in the leftmost slot of the lead's tab.

## Behaviour

`board::open` (`src/board.rs`, currently ~line 140) keeps: the `launch::launched` child skip, the ledger lock, the per-tab registry (`boards/<tab>.json`) with `terminal_id` validation and reuse. Replace only the placement:

1. Read the lead tab's layout (`herdr pane layout` for the owner pane, or socket `pane.layout`/`layout.export`). Find the leftmost top-level pane: walk the tree's `first` children from the root while splits are `right`; for a `down` split take its top (`first`) subtree and keep walking. If the owner is the only pane, that is the owner.
2. `herdr plugin pane open --plugin dogsled --entrypoint board --placement split --direction right --target-pane <leftmost> --no-focus`.
3. `herdr pane swap` the new pane with `<leftmost>` (explicit form: `source_pane_id`/`target_pane_id`), so the dock is on the left. Swap preserves PTYs.
4. Set the new split's ratio so the dock is 38 cells: `ratio = 38 / split_rect.width`, clamped to (0, 1). Use `layout.set_split_ratio { tab_id, path, ratio }`; find `path` (list of `first`/`second` steps, as Herdr defines it) by locating the split whose first child is the dock pane in `layout.export`. This method is socket-only: add `src/herdr_socket.rs`, a small blocking client over `HERDR_SOCKET_PATH` (Unix socket, newline-delimited JSON request/response per the Socket API doc). No new crates unless one already in `Cargo.lock` covers it.
5. If the leftmost column is split top/bottom, the dock takes the top slot. Never bounce or re-create other panes; never call `layout.apply` (it kills PTYs).

Toggle: add `[[actions]] id = "toggle"` in `herdr-plugin.toml` running a new `dogsled board --toggle` (owner from `HERDR_PANE_ID`/the action context — check what Herdr gives a plugin action in the Plugins doc). Toggle closes this tab's registered dock if it is live, else opens it as above. Update the manifest `description` (no longer "right-side").

Errors: any Herdr call failure returns `Error::Invalid` with the Herdr message; do not fall back to the old right split.

## Scope

Owned paths: `src/board.rs` (`open` and a new `toggle` only; leave rendering code alone), `src/herdr_socket.rs` (new), `src/main.rs`/`src/cli.rs` (the `--toggle` flag only), `herdr-plugin.toml`, `tests/` (new dock tests), `README.md` (the board-placement sentence only), `CHANGELOG.md` (one Unreleased entry, following its existing style).
Generated paths: none.
Read first: `src/board.rs` `open`, `src/shell.rs` (the `Shell` trait and its fake used in tests), `tests/plugins.rs`, the existing board-open tests (`grep -rn "fn .*open" src/board.rs tests/`), Herdr docs: Socket API https://raw.githubusercontent.com/herdrdev/herdr/v0.9.5/docs/next/website/src/content/docs/socket-api.mdx , CLI https://raw.githubusercontent.com/herdrdev/herdr/v0.9.5/docs/next/website/src/content/docs/cli-reference.mdx , Plugins https://raw.githubusercontent.com/herdrdev/herdr/v0.9.5/docs/next/website/src/content/docs/plugins.mdx . Reference for docking order only (read, do not copy code): https://github.com/alexarthurs/herdr-sidebar `plugins/herdr-sidebar/src/ensure.rs` ~740-840.

## Checks

Acceptance (you run it): `cargo test --locked board` — new tests pass, using the fake shell and a fake Unix-socket server in a temp dir:
- single-pane tab: open → split targets the owner, swap, `set_split_ratio` with `38/width`;
- three columns: targets the leftmost, not the owner;
- leftmost column split top/bottom: targets the top pane;
- reuse: a live registered dock returns the saved pane with no Herdr writes;
- toggle closes a live dock and opens when none;
- a launched child is skipped.

Allowed light checks: `cargo test --locked board`, `cargo test --locked herdr_socket`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`. The lefthook pre-commit hook runs `mise run check:rust` on commit; do not bypass it.
Forbidden heavy checks: `mise run check`, release builds, `herdr plugin install`, anything against the user's live Herdr session.

## Publication authority

Commit: yes. Push `feat/sidebar-dock` to origin: yes. Draft PR: yes. Merge, release, tag, plugin install: no.

Failure behaviour: if Herdr's API cannot express a step as written (for example `path` semantics differ), stop and report the exact doc/source evidence; do not substitute `pane resize` or `layout.apply`.

## Reply

Branch, head SHA, draft PR URL, changed paths, commands with results, unverified scope (live Herdr behaviour is unverified by design; say so).
