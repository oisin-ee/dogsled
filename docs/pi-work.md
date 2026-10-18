# Pi work display contract

Wrangle owns the work strip. pi-todo owns the tasks and their session-branch history.
The two extensions communicate through `pi.events`. Neither imports the other package.

## Version 1 events

| Event | Sender | Data |
| --- | --- | --- |
| `oisin.todo.ready` | pi-todo | `version`, `publisher` |
| `oisin.work.request` | Wrangle | `version`, `presenter`, `request` |
| `oisin.todo.snapshot` | pi-todo | Request identity, `publisher`, `revision`, `session`, `branch`, `tasks` |
| `oisin.work.claim` | Wrangle | Request identity, `publisher`, `revision` |
| `oisin.work.release` | Wrangle | Request identity |
| `oisin.todo.stop` | pi-todo | `version`, `publisher` |
| `oisin.work.open` | pi-todo | Request identity and `view: "todos"` |

`version` is `1`. Request identity contains `version`, `presenter`, and `request`.
Each presenter and publisher has a new identifier for each session lifecycle.
A new request invalidates earlier replies. Revisions increase within a publisher lifecycle.
The snapshot identifies the current Pi session and active branch leaf, including a null leaf.
Each task has `content` and a status: `pending`, `in_progress`, `completed`, or `abandoned`.

Both extensions subscribe during `session_start`. Wrangle requests a snapshot at startup and when a provider announces readiness.
Thus either session-handler order works. Wrangle validates snapshots and draws before acknowledging presentation ownership.
Only then does pi-todo remove its widget. Shutdown, reload, and rejected snapshots release presentation ownership.
The provider restores its standalone widget on release. Todo commands remain the only task-state writers.

The strip uses Wrangle's existing board refresh. The inbox reader remains the only completion-delivery owner.
Todos follow the Pi branch. Agent and job rows follow the calling Herdr owner pane.
The display does not create a new job database or imply that a new Pi session resets pane-owned jobs.

## Work panel

`/work` opens All, Todos, Agents, and Jobs tabs. Tab and Shift+Tab change tabs.
Arrow keys move selection. Enter opens details. Escape returns to the editor.
Todos are read-only. `/todos` and Alt+T open the Todos tab when the provider is claimed.

Job history uses the existing refresh coordinator while the panel is open.
It joins `items --owner` and `ps --all` by host ID and job ID.
`machine list` maps board host labels to IDs for selection and deduplication.
Completed history starts collapsed. No extra timer or execution store is created.

Job details use the bounded tail from `read`. The full `logs` command is shown separately.
The panel does not run `logs --follow` or wait for jobs.
Stop and cancel need confirmation. Agent messages use the inbox, not terminal input.
Session shutdown cancels open interactions and prevents late dialog responses from executing actions.

## Contract acceptance

From the Wrangle source checkout, run the real provider/consumer integration test:

```sh
PI_TODO_SOURCE=/path/to/oisinpi/packages/pi-todo/src node --test pi/work-contract.test.ts
```

Build pi-todo's existing WASM bindings before this test. The normal shim suite exercises protocol rejection and rendering without that checkout.
