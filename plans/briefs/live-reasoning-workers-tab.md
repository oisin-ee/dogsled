# LR2: Workers tab on the lead's page

Plan: `plans/lead-workers-tab.md` (owner-approved 2026-11-06). Base: `origin/main` `aef6cce`.
Read the plan and mockups with `git show origin/docs/portal-mocks:<path>`:
`plans/lead-workers-tab.md`, `.impeccable/mocks/lead-workers-tab/lead.html`, `worker.html`, `lead-1600.png`, `worker-1600.png`.

## Outcome

A lead's workers leave the agents sidebar and appear in a Workers tab on the lead's page; a worker opens in the conversation column with a link back to its lead.

## Cause

- `web/src/board.rs:111-128` nests workers under a lead only when `lead.host == row.host`. A local lead's netcup worker does not nest, because `RowView` (`portal-types/src/lib.rs:94`) and `board::Row` (`src/board.rs:460`) have no owner host.
- `web/src/board.rs:248-270` (`RepoGroup`) renders nested workers as `SidebarMenuSub` rows.
- `web/src/agent_view.rs:358-364` builds `workers` from every board row, not the selected lead's.
- `web/src/agent_view.rs:524-534, 590-600` has only Chat and Todos tabs.

## Edits

1. Owner host. Add `owner_host: Option<String>` (a host label) to `board::Row` and `RowView` (serde default, skip if none). It is the label of the host whose ledger records the item: `local` for rows built from this host's items, the peer's label for rows that come from a peer. Find each `owner_pane: Some(...)` site in `src/board.rs` (lines 586, 686, 760, 807, 927) and set it there. Map it in `src/web/board.rs:125-147`. Nesting in `web/src/board.rs` matches `lead.host == row.owner_host.unwrap_or(row.host)` and `lead.pane == row.owner_pane`.
2. Sidebar (`web/src/board.rs`): `RepoGroup` stops rendering `SidebarMenuSub`. `NavigationAgent` for a lead with workers adds one line: one dot per worker and `N workers · M working`. Dot states: green working, hollow idle, dim finished, yellow waiting on the owner (prompt or `unseen`). Clicking the line selects the lead and sets the Workers panel. The repository count counts leads only. An orphan worker stays in the sidebar as today. Move the prompt check at line 282 to the worker rows; the summary rolls it up.
3. Lead page (`web/src/agent_view.rs`, new `web/src/workers.rs`):
   - Add `LeadPanel::Workers` in `web/src/app.rs`. At 1400px and wider the right column gets a tab control, `Todos d/t` and `Workers n`, with the existing segmented-tab component. Below 1400px the header tabs become `Chat`, `Todos`, `Workers`; below 1100px `Agents` comes first, as now. Todos opens by default; the last choice holds while the page stays open.
   - `workers` takes only the rows whose owner matches the selected lead. Closed rows still show, as Finished, until they leave the board.
   - Groups, in this order: `Waiting on you` (prompt or `unseen`, yellow), `Working` (working or running), `Idle`, `Finished` (closed or done). Do not add a "Held by the lead" group; no signal exists for it.
   - Each row has three lines: title; state with elapsed time (`quiet_ms` or `age_ms`) and host; `row.activity`, two lines at most. The column header shows the working count.
4. Worker view: a worker opened from the tab shows a back button and its title in the header. The meta line reads `Worker of <lead title> · <what> · <host>`, and the lead title selects the lead. The composer placeholder is `Message the worker…`. The sidebar keeps the lead row active, and the Workers tab marks the open worker.
5. `web/assets/console.css`: use the `DESIGN.md` tokens only. `PRODUCT.md` principle 7: workers sit on their lead's page.
6. Tests: cross-host nesting (local lead, netcup worker) in `web/src/board.rs`; owner host for local and peer rows in `src/board.rs`; component tests for the summary line, the four groups, lead filtering, and the worker header in `web/src/component_tests.rs`.

## Paths

Owns: `src/board.rs`, `src/web/board.rs`, `portal-types/src/lib.rs`, `web/src/board.rs`, `web/src/agent_view.rs` (header, tabs, right column, `workers`), new `web/src/workers.rs`, `web/src/app.rs`, `web/src/main.rs` module list, `web/assets/console.css`, `web/src/component_tests.rs`, `PRODUCT.md`.
Reads: `src/`, `web/`, `DESIGN.md`. Do not change `Conversation`, `ToolRecords`, or `LiveActivity` in `agent_view.rs`; LR3 owns them.

## Acceptance and checks

`cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`, `cargo test -p web`, `cargo clippy -p web --target wasm32-unknown-unknown -- -D warnings` pass. (`cargo` needs `web/public`; run `moon run dogsled:web-build` first if it is missing.) The lead does the browser check.
Open a PR to `main`.
