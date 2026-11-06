# Lead Workers tab

## Scope and status

Status: mockup for owner review. No production code changed.

On 2026-11-06 the owner chose **a separate Workers tab** on the lead's page.
The rejected alternative put workers inside the Todos list as one combined Work panel.

The change moves a lead's workers out of the agents sidebar and into the lead's own page.
It supersedes the worker rows that `plans/conversation-live-work-mockups.md` nests under each lead in the sidebar.
The one-stream conversation, the live reasoning, and the inbox do not change.

## The design

### Agents sidebar: leads only

- A lead row keeps its title, phase, and newest thought or command.
- Below that, one summary line shows a dot for each worker and a count, such as `5 workers · 2 working`.
  - Green dot: working. Hollow dot: held or idle. Dim dot: finished. Yellow dot: waiting on the owner.
- Clicking the summary opens the lead's page with the Workers tab selected.
- The repository count counts leads, not workers.
- A worker whose lead is gone has no lead page. It stays in the sidebar under its repository, as today.

### Lead page: Workers tab

At 1400px and wider, the right column gets a tab control: `Todos 2/6` and `Workers 5`.
The planned Files tab (`plans/git-files-sidebar.md`) becomes a third tab in the same control.
Below 1400px, the header tabs become `Chat`, `Todos`, and `Workers`. Below 1100px, `Agents` comes first, as shipped.

The Workers tab groups the lead's workers by what they need:

1. **Waiting on you**: a key prompt, or idle and unseen while the lead has not picked it up. The group and the sidebar dot turn yellow.
2. **Working**: running or thinking.
3. **Held by the lead**: idle because the lead told it to wait.
4. **Finished**: done, with the time the lead accepted the result.

Each row has three lines: the title, the phase with elapsed time and host, and the newest thought or command (two lines at most).
The column header shows the working count.

### Worker view

Clicking a worker row opens the worker's stream in the conversation column.

- The header has a back button and the worker's title.
  The meta line reads `Worker of <lead title> · model · host`, and the lead title is a link.
- The composer sends to the worker: `Message the worker…`.
- The sidebar keeps the lead row selected. The Workers tab marks the open worker.
- Lead–worker messages appear in both streams as attributed cards, for example `Live Activities for Practice and Event · worker message`.

## Artifacts

All files are in `.impeccable/mocks/lead-workers-tab/`:

- `lead.html`: the rondo lead's page with the Workers tab open.
- `worker.html`: the Live Activities worker opened from that tab.
- `lead-1600.png`, `lead-1280.png`, `lead-900.png`, `worker-1600.png`, `worker-1280.png`, `worker-900.png`: live edge at 1600 × 1000, 1280 × 800, and 900 × 800.
- `lead-1600-history.png`: the top of the lead stream, with worker messages and the lead's replies.
- `lead-1600-todos.png`: the same page with the Todos tab selected.

Add `?still` to stop the playback, `?top` to open at the top of the stream, and `?todos` to select Todos.

Review through Impeccable:

```sh
/Users/oisin/.pi/agent/skills/impeccable/scripts/impeccable serve-question --start --payload .impeccable/mocks/lead-workers-tab/review.json
```

### Provenance

Both streams replay real Pi sessions from 6 November 2026, frozen at 13:24:05 UTC:

- Lead: the rondo claude-opus-5-5 session `~/.pi/agent/sessions/--Users-oisin-dev-rondo--/2026-11-05T11-24-18-793Z_01a1ab73-….jsonl`, 13:20:48–13:24:20.
- Worker: the Live Activities gpt-6.1-sol session in `~/.local/state/dogsled/launches/r-1793970907483-64730-0.sessions/`, 13:15:44–13:24:09.

Worker titles, the hold message, and the accepted results come from the lead's messages and todo records.
The newest thought of each stream types in at the live edge.
The notifications worker's phase and snippet, the dogsled row, and the host meters are illustrative.
No image model made these rasters.

## Implementation notes

- `web/src/board.rs` `navigation_groups` already nests ledger rows under their lead as `NavigationRow.workers`.
  `RepoGroup` stops rendering `SidebarMenuSub`, and `NavigationAgent` renders the summary line instead.
- `web/src/agent_view.rs` builds `workers` from every board row. It must take only the selected lead's workers.
- Nesting matches `lead.host == row.host && lead.pane == row.owner_pane`. `RowView` has no owner-host field.
  A worker that a local lead placed on netcup may not nest. Verify this before building. If it fails, the projection needs `owner_host`.
- Todos resolve only for `host == "local"`. A remote lead gets the Workers tab but no todo counts.
- Worker snippets need the live-reasoning protocol change in `plans/conversation-live-work-mockups.md`.
  Without it, version one shows phase and elapsed time only.
- The "Held by the lead" group needs a signal. Today a held worker is idle, the same as a worker that waits on the owner.
  The worker's newest inbox message from its lead is one possible source. This is an open decision.
- The prompt check in `NavigationAgent` (`!row.worker || kind == Prompt`) moves to the Workers tab rows. The sidebar summary rolls it up.
- `PRODUCT.md` principle 7 says workers sit under their lead in the list. Reword it to say they sit on the lead's page.

## Open questions for the owner

1. Which right-column tab opens by default on a lead: Todos, Workers, or the last one used?
2. Should a finished worker stay in the tab until its branch lands, or until the lead accepts its result?
3. The conversation is still 772px wide at 1600px. This design does not add width.

## Before implementation

1. The owner approves this direction or asks for changes.
2. Draw the accepted layout in `plans/lead-workers-tab.squig.json`.
3. Check cross-host nesting and decide the "held" signal.
4. Build with the existing Dioxus components. These HTML files are design artifacts only.
