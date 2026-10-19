# Pi work display contract

Wrangle owns the above-editor work widget. pi-todo owns the tasks and their session-branch history.
The two extensions communicate through `pi.events`. Neither imports the other package.

## Version 2 events

| Event | Sender | Data |
| --- | --- | --- |
| `oisin.todo.ready` | pi-todo | `version`, `publisher` |
| `oisin.work.request` | Wrangle | `version`, `presenter`, `request` |
| `oisin.todo.snapshot` | pi-todo | Request identity, `publisher`, `revision`, `session`, `branch`, `display` |
| `oisin.work.claim` | Wrangle | Request identity, `publisher`, `revision` |
| `oisin.work.release` | Wrangle | Request identity |
| `oisin.todo.stop` | pi-todo | `version`, `publisher` |
| `oisin.work.open` | pi-todo | Request identity and `view: "todos"` |

`version` is `2`. Request identity contains `version`, `presenter`, and `request`.
Each presenter and publisher has a new identifier for each session lifecycle.
A new request invalidates earlier replies. Revisions increase within a publisher lifecycle.
The snapshot identifies the current Pi session and active branch leaf, including a null leaf.
`display` contains `empty`, `summary`, `current`, `text`, opaque `state`, and a `createView` function.
Pi-todo owns these projections. Wrangle does not interpret task records or reproduce todo rendering.
The capability is in-process only. Never serialize functions or components into session state.

`createView({ theme, height, requestRender })` creates a view for one inline expansion.
The view exposes `render(width)`, `handleInput(input)`, `invalidate()`, `update(state)`, and idempotent `dispose()`.
Wrangle forwards navigation, opaque state updates, theme invalidation, and cleanup.
The same pi-todo renderer supplies standalone and hosted rows. All shows one summary entry that opens Todos.

Before claiming a snapshot, Wrangle creates, updates, renders, and disposes a preflight view.
A live view survives ordinary refreshes and ends on collapse, publisher replacement, release, or shutdown.
Factory, render, update, and input failures release the claim and restore standalone todos.
Version 1 peers do not receive flattened data. Incompatible versions leave standalone todos available.

Both extensions subscribe during `session_start`. Wrangle requests a snapshot at startup and when a provider announces readiness.
Thus either session-handler order works. Wrangle validates snapshots and draws before acknowledging presentation ownership.
Only then does pi-todo remove its widget. Shutdown, reload, and rejected snapshots release presentation ownership.
The provider restores its standalone widget on release. Todo commands remain the only task-state writers.

The widget uses Wrangle's existing board refresh. The inbox reader remains the only completion-delivery owner.
Todos follow the Pi branch. Agent and job rows follow the calling Herdr owner pane.
The display does not create a new job database or imply that a new Pi session resets pane-owned jobs.

## Inline work widget

The widget above the editor is the wrangle board: a header with counts and a load meter for each host,
then the current task, then agents, jobs, and queued work grouped TO LAND, ATTENTION, RUNNING, and WAITING.
Each row shows the harness and model, host, activity, and age in aligned columns.
Alt+W and `/work` expand or collapse the same widget. Neither opens a popup or replaces the editor.
The expanded widget keeps the board's header, groups, and columns, and adds a view line, a selection marker, and key hints.
The expanded widget uses at most half the terminal height, except on terminals shorter than eight rows.
Typing and ordinary editor keys remain with the editor.

Alt+J/K moves the selection. Alt+] changes between All, Todos, Agents, and Jobs.
Alt+I shows details in the same widget, or returns to the list. Alt+J/K scrolls details.
In Todos, pi-todo handles Alt+J/K scrolling and Alt+I expansion. Full task text wraps within that viewport.
Alt+S stops a job, Alt+C cancels queued work, and Alt+M sends an agent message.
Alt+R refreshes the work data. Stop and cancel still require confirmation.
Todos are read-only. `/todos` and Alt+T expand the Todos view when the provider is claimed.

Job history uses the existing refresh coordinator while the widget is expanded.
It joins `items --owner` and `ps --all` by host ID and job ID.
`machine list` maps board host labels to IDs for selection and deduplication.
Completed history starts collapsed. No extra timer or execution store is created.

Job details use the bounded tail from `read`. The full `logs` command is shown separately.
The widget does not run `logs --follow` or wait for jobs.
Stop and cancel need confirmation. Agent messages use the inbox, not terminal input.
Session shutdown cancels open interactions and prevents late dialog responses from executing actions.

## Contract acceptance

From the Wrangle source checkout, run the real provider/consumer integration test:

```sh
PI_TODO_SOURCE=/path/to/oisinpi/packages/pi-todo/src node --test pi/work-contract.test.ts
```

Build pi-todo's existing WASM bindings before this test. The normal shim suite exercises protocol rejection and rendering without that checkout.
