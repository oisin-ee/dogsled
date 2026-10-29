# Pi work display contract

Dogsled owns the above-editor work widget. pi-todo owns the tasks and their session-branch history.
The merged extension connects both parts through typed, in-process capabilities from `createTodo(pi, options)`.

## Direct display interface

`TodoWork.attach(presenter)` returns a detach function. The presenter exposes `show(snapshot)`, `open()`, and `stop()`.
`show` returns `true` after Dogsled renders the snapshot. Pi-todo then claims shared presentation and removes its standalone widget.
A `false` result releases the claim. Detachment, shutdown, and rendering failures restore standalone todos without changing task state.

A snapshot contains `revision`, `session`, `branch`, and `display`. Revisions increase within each provider session.
The snapshot identifies the current Pi session and active branch leaf, including a null leaf.
Dogsled rejects snapshots from another session or branch and ignores duplicate or older revisions.
`display` contains `empty`, `summary`, `current`, `text`, `sections`, opaque `state`, and a `createView` function.
`sections` contains four lists in order: `in_progress`, `pending`, `completed`, `abandoned`. Each list contains `{status, count, items}`.
An item contains `{status, text, description?, review, unit, children}`. A child contains `{status, text, description?, unit}`.
The sidebar draws these lists. Pi writes them to the ledger as `{session, branch, revision, current, pane, sections}`.
Pi-todo owns these projections. Dogsled does not interpret task records or reproduce todo rendering.
The capability stays in-process. Functions and components never enter session state.

`createView({ theme, height, requestRender })` creates a view for one inline expansion.
The view exposes `render(width)`, `handleInput(input)`, `invalidate()`, `update(state)`, and idempotent `dispose()`.
Dogsled forwards navigation, opaque state updates, theme invalidation, and cleanup.
The same pi-todo renderer supplies standalone and hosted rows. All shows one summary entry that opens Todos.

Before claiming a snapshot, Dogsled creates, updates, renders, and disposes a preflight view.
A live view survives ordinary refreshes and ends on collapse, detachment, or shutdown.
Factory, render, update, and input failures release the claim and restore standalone todos.

Both parts start during `session_start`. Attachment publishes an already started provider. Provider startup publishes to an attached presenter.
Thus either session-handler order works. Todo commands remain the only task-state writers.

The widget uses Dogsled's existing board refresh. The inbox reader remains the only completion-delivery owner.
Todos follow the Pi branch. Agent and job rows follow the calling Herdr owner pane.
The display does not create a new job database or imply that a new Pi session resets pane-owned jobs.

## Inline work widget

The widget above the editor is the dogsled board: a header with counts and a load meter for each host,
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

From the Dogsled source checkout, run the real provider/consumer integration test:

```sh
node --test pi/work-contract.test.ts
```

Build pi-todo's existing WASM bindings before this test. The normal shim suite exercises protocol rejection and rendering without that checkout.
