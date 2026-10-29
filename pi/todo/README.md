# @oisin-ee/pi-todo

A `todo` tool for [pi](https://pi.dev). The model keeps one ordered list of outcomes and bounded subtasks for the session. Multiple units can be active together.
The list rules run in Rust, compiled to WebAssembly.

```bash
pi install npm:@oisin-ee/pi-todo
```

To install it with the other packages, use [`@oisin-ee/oisinpi`](https://www.npmjs.com/package/@oisin-ee/oisinpi).

## The tool

The model calls `todo` with one `op`. Tasks are addressed by their exact content.

| `op`     | Effect                                                          |
| -------- | --------------------------------------------------------------- |
| `init`   | Replaces the list with `items`.                                 |
| `append` | Adds `items` to the end.                                        |
| `start`  | Explicitly activates `task` without demoting other active work. |
| `done`   | Completes `task`. Without `task`, completes every open task.    |
| `drop`   | Abandons `task`. Without `task`, abandons every open task.      |
| `view`   | Returns the list. It does not change it.                        |

Creation and append leave tasks pending. Nothing starts automatically. This replaces the previous single-active, automatic-start behavior.
Use `start` to activate or reopen work explicitly. Starting a child also opens its parent. Starting a parent does not start its children.

Keep `content` a short headline: aim under 60 characters, imperative, no trailing detail. Put context, approach, or acceptance detail in the optional `description` (one or two sentences). Outcomes and subtask drafts accept `description`; a plain string item has none. A blank description counts as none. `update` with `description` sets it on an open task or subtask, and a blank value clears it. It never changes `content` or how a task is selected. `view` prints a description on the line under its headline; the Pi widget shows headlines only.

`items` accepts strings or outcomes with string subtasks:

```json
{
  "op": "init",
  "items": [
    {
      "content": "Password reset",
      "subtasks": ["Implement tokens and behavior tests", "Implement form and interaction tests"]
    },
    "Update documentation"
  ]
}
```

To target a child, supply its exact `task` content and its exact root `parent` content:

```json
{ "op": "start", "parent": "Password reset", "task": "Implement tokens and behavior tests" }
```

To add children, use `append` with `parent` and string `items`. Only two levels are permitted.
Root names are unique. Child names are unique within their parent. Different parents can use the same child name.
Blank names, duplicate siblings, unknown targets, grandchildren, and malformed requests fail without saving a change.

Each executable unit fits one agent session, with one deliverable, clear scope, and an acceptance condition.
Split independent outcomes or work that needs another session. Combine trivial steps with their deliverable and tests.
Delegate independent units separately. The lead can execute one unit. Keep dependent units ordered.
Do not delegate an oversized parent as one assignment.

A parent stays open after its children close and shows **Ready for review**.
Complete the parent explicitly after integration review. Completing a parent with open children fails, including bulk completion.
Dropping a parent abandons open children but preserves completed and abandoned children.
Untargeted `done` and `drop` affect open work only. A parent without `task` is not a bulk-child selector.
Appending children to a closed parent reopens it as pending. An active parent stays active.
Outcome and subtask progress remain separate. Abandoned work never counts as completed.

## Activation gate

Shell, code, and mutation tools need one active local unit. Structured reads, workflow control, and `todo` calls stay available without one.
The gate checks the selected model at each call. The planner selections `tier/plan-claude` and `tier/plan-gpt` skip the activation check, so a planning session can read and inspect without a unit.
Every other selection needs an active unit, including a tier with another name and a physical model that only looks like a planner.
The exemption is a workflow prerequisite, not a permission. It does not grant write access, and other policies such as guardrails still apply.
An active unit stays required for the sessions that implement code.

## State

Each task change is saved as a `todo-state` entry on the session branch. A resumed session, a `/tree` move,
and calls made from inside codemode all see the list of the current branch.
Old flat snapshots retain their recorded statuses and read with empty subtasks.

## Widget

The widget shows only in-progress tasks by default. Active tasks use the theme accent and bold text.
Long task names wrap to the terminal width. An empty list has no widget.

Press **Alt+T** to expand Upcoming, Done, and Dropped sections beneath the active task. Press **Alt+T** again to collapse them.
`/todos` also toggles these sections. `/todos list` prints the full list.

Expansion stays open through task updates. Session changes and `/tree` navigation collapse it. Expansion does not change saved tasks.
When no task is active, a short status and the shortcut hint remain visible.
Outside the terminal UI, `/todos` prints the list instead.

## Shared work display

The merged Dogsled extension attaches its work widget directly to pi-todo.
Tasks remain on the current Pi session branch. Dogsled receives a typed, read-only presentation capability.
Pi-todo owns the renderer in both standalone and shared views. Dogsled hosts it above the editor without opening a popup.
Alt+J/K scrolls the hosted rows. Alt+I toggles their expansion. Ordinary editor keys remain unchanged.
`/todos` and Alt+T request the shared Todos view. `/todos list` still prints the full list.
When shared presentation ends, the standalone widget returns without changing task state.

The [display contract](https://github.com/oisin-ee/dogsled/blob/main/docs/pi-work.md) defines direct attachment, snapshots, claims, and release.
`display.sections` contains four typed status groups in order: `in_progress`, `pending`, `completed`, `abandoned`. Each group contains a count and its items.
Each item carries its text, optional description, review flag, contract summary, and subtasks with their optional descriptions.
The core owns this grouping. A presenter only draws it. Rendering failures restore standalone todos without changing saved tasks.

## Dogsled plans

After the user approves a plan, Dogsled plan mode (`/plan` and `submit_plan`) calls the todo approval handle directly.
Pi-todo continues the approved plan in the same session. No setting is needed.

After approval, the agent reads the current todo list and adds the remaining approved work before implementation.
It preserves unrelated tasks and completed progress. Free-form plans do not need a special checklist format.
The todo tool owns progress, persistence, reminders, and the shared work display.
A different working directory or another owner of the `todo` tool stops the handoff with an error.

## Reminder

When the agent stops with open tasks, pi-todo adds a visible reminder to the conversation and continues the turn.
The reminder names open children and parents that await integration review.
It sends at most 3 reminders per user message. After a reminder, it waits for the list to change before it sends
another. A new user message resets the count.

## Other todo tools

If another package already registers `todo`, pi-todo stays inactive and shows a warning that names that package.

## Source

The original flat operations and the reminder derive from
[oh-my-pi](https://github.com/can1357/oh-my-pi/tree/e7fe318/packages/coding-agent/src) (`tools/todo`,
`todo-tracker.ts`), MIT. Saving a custom entry on every change follows
[rpiv-todo](https://github.com/juicesharp/rpiv-mono/tree/main/packages/rpiv-todo).
