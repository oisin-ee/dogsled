# Combined sidebar

Status: requested by the owner, 2026-11-19. Wireframe: `plans/combined-sidebar.squig.json`.

## Goal

The portal has one sidebar. The separate "Waiting on you" column is removed. Agents that need the owner move to the top of the agents sidebar, as in the Claude and Codex apps.

This supersedes the 2026-11-02 steer "Inbox and agents are two left sidebars" in `.impeccable/surfaces/web.md`.

## Behaviour

1. The sidebar is 280px on the raised panel. Its header keeps `Agents n`, Metrics and New.
2. A "Needs you" section sits at the top of the list. Its yellow badge shows the server's inbox count.
3. The section has one row per agent with inbox items, newest item first. A row shows a yellow dot, the agent title and the age. Its second line shows the kind icon, the kind and the item title, and `+n` for more items.
4. A key prompt shows its keys inline under the row. A lead with workers keeps its worker summary.
5. Selecting a row opens the agent with its newest item selected. The open row has a yellow left edge.
6. A waiting agent leaves its repository group while it waits. A waiting worker nested under a lead also stays in the lead's worker list.
7. An inbox item whose agent has no navigation row still gets a row, from the item's agent title and repository.
8. With nothing waiting and no error, the section is absent. When the inbox cannot be read, the section stays with its error and a `—` count.
9. The sidebar never folds. The `Agents` tab below 1100px is removed. A plan review takes everything right of the sidebar.

## Code

- `web/src/board.rs`: `take_waiting` moves waiting agents out of `Navigation`.
- `web/src/app.rs`: `NeedsYou` renders the section; the `InboxSidebar` column and `LeadPanel::Agents` are removed.
- `web/src/agent_view.rs`, `web/src/metrics.rs`: the `Agents` header tab is removed.
- `web/assets/console.css`, `web/assets/review.css`: a two-column grid.
