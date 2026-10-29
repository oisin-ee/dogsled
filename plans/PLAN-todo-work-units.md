# Todo work units and subtasks

## Execution evidence and release handoff

Source implementation is locally committed on both main branches. Native todos track remaining acceptance, not this plan's original checklist.

- Oisinpi: hierarchy `eb674408`, closed-parent invariant `4e63e13`, strict parsing integration `58abb27`, final test organization `478ef47`. Wrangle: hosted renderer `076382e`, hierarchy contract coverage `3207bb4`.
- Final behavior proof: 79 TypeScript/WASM tests passed before test-only regrouping, then all 19 hierarchy tests passed after regrouping. Eleven core tests passed in the worker. Both affected Clippy checks, Rust formatting, TypeScript lint, and workspace formatting passed.
- Real provider/consumer integration passed against final source in both load orders, including nested rows and explicit parent review. Real Pi regular/100-column and fullscreen/55-column runs passed after strict parsing integration. Actual tool calls created pending children, started two siblings, completed children, then completed the parent separately. Inline navigation preserved a checked editor draft. Smoke processes exited.
- Independent static review found no Unit 1 lifecycle defects. Unit 2 review exposed missing closed-parent validation and unknown-field handling. Rust now rejects both through its own validator and strict map deserializers. Caller tests prove rejected inputs write no state.
- Release requirement: publish a protocol-v2 Wrangle release and matching pi-todo release together, then update collection dependency pins through the normal release process. No release version is assigned here. Existing installed oisinpi 0.5.1 and Wrangle 0.12.2 belong to the separate inline-widget release and do not contain this feature. Mixed protocol versions retain standalone todos.

No feature publication, installation, dependency-pin update, or managed configuration change is authorized by this handoff. Untested acceptance details remain: manual theme switching and resumed-session UI navigation for nested state. Existing branch and theme tests cover those seams but do not replace a manual walkthrough.

The main worktree's stale `core.hooksPath` pointed at the old `pi-prewalk` checkout. It now points at this worktree's generated `.moon/hooks`. Final commit hooks executed successfully.

## Context

The user wants todos that guide agents toward bounded delegation. One assignment must fit one agent session, without trivial fragmentation or oversized handoffs.

The current tool accepts a flat string list. It has no parent relationship or task-sizing guidance. The owning code is `crates/todo-core` and `packages/pi-todo/src/index.ts`.

Approved for implementation. The user subsequently selected Wrangle's new inline widget design as the host. Preserve that work and its controls. Publication and installation remain separately gated.

## Approach

Confirmed: use guidance, not delegation enforcement. The plugin will not block tool calls, require dispatch records, or automatically spawn agents.

Confirmed hierarchy: two levels only. A todo represents an outcome. Its optional subtasks are bounded work units, not procedural steps. An already-small todo can stand alone.

- Represent larger outcomes with parent todos and bounded work units with subtasks.
- Describe a work unit through one deliverable, a clear scope, and an acceptance condition.
- Prompt the agent to split oversized assignments before delegation and combine trivial steps into meaningful units.
- Show the hierarchy in model output and the existing todo views.
- Keep todo state on the current session branch. Do not introduce a second progress database.

Confirmed: multiple work units can be in progress together. The lead can execute one bounded unit while sub-agents handle other independent units.

Each subtask includes implementation and its tests where applicable. Do not create separate delegated units for reading files or other trivial steps.

Task-sizing guidance belongs in the tool description, creation responses, and approved-plan handoff. Describe scope and acceptance conditions in prompts, not mandatory dispatch metadata.

### Compatibility findings

Current local source was inspected at `131adc5`. The remote reader reported an older implementation without Plannotator or the current widget. Those claims do not describe this checkout.

- `Task` and `TodoList` reject unknown fields. Existing flat snapshots need explicit support in the new parser.
- Task content currently acts as identity. Retain exact-content addressing, scoped by an optional parent for subtasks.
- Normalization currently permits one active task globally. Replace that rule with explicit starts that preserve other active units.
- `PlannotatorHandoff.continuation` already instructs the agent to preserve existing work and add approved tasks. Extend that guidance rather than introducing another handoff.
- `lifecycle.replay` uses structured widget rows. `TodoPresentation` and `TodoWork` own current presentation integration, not the older flat widget described by the reader.

`TodoWork.publish` currently sends `list.tasks` directly through the version 1 shared-display contract. Nested state must not silently change that external contract.

The published [Wrangle display contract](https://github.com/oisin-ee/wrangle/blob/main/docs/pi-work.md) defines flat tasks with content and status. It has no parent field. Native shared-panel nesting is outside this repository's current contract.

Confirmed: pi-todo owns reusable todo rendering. Wrangle hosts it inside the shared panel. Qualified-label flattening is rejected.

`PLAN-unified-work-panel.md` records the original decision: Wrangle owns the shared strip and panel, while pi-todo supplies task snapshots. It deliberately avoided a package dependency and retained the standalone widget separately. Current `TodoPresentation.claim` hides that widget after acknowledgement.

That separation avoids duplicate displays but duplicates todo presentation. Hierarchy exposes the cost: changes to the todo widget do not reach the shared panel.

Correction to the earlier scope assumption: the existing plan identifies Wrangle as first-party source at `/Users/oisin/dev/wrangle`. A source-level integration change is possible with user scope approval. Installed dependencies must remain untouched.

Wrangle owns the containing panel and agent/job views. Source inspection at Wrangle `313f021` confirms duplicate todo labels in `pi/work-panel.ts` and counts in `pi/work.ts`. Replace those projections with pi-todo-owned presentation.

The existing `TodoWidget` extends `Text`, while `WorkPanel` uses `SelectList`. Use a provider-owned interactive adapter around the same todo renderer. Leave the existing untracked Wrangle `plans/` directory untouched.

### Shared rendering contract

Extend the existing event lifecycle as protocol version 2. Publish pi-todo-owned summaries and an in-process view factory instead of asking Wrangle to interpret task records. Retain publisher, session, branch, request, and revision validation. No cross-package imports, new shared package, or generic provider registry.

The factory receives the host theme, keybindings, viewport limits, and render callback. It creates an interaction-local component with `render(width)`, `handleInput`, `invalidate`, `update`, and idempotent `dispose`. The provider owns the snapshot passed to its component. Wrangle forwards opaque presentation updates without interpreting task hierarchy.

- Reuse the same todo row renderer in the standalone widget and hosted Todos view. Pi-todo owns indentation, status marks, progress, expansion, scrolling, and full-text detail.
- Wrangle owns the inline widget chrome, refresh, and Alt-key shortcuts. Forward Alt+J/K navigation and Alt+I detail/back to the hosted todo component. Alt+] changes views and Alt+W collapses the widget. Plain editor keys retain their existing behavior.
- The All view shows a compact provider-owned summary entry that opens Todos. Do not retain a second selectable todo list in Wrangle. Agent/job entries and actions remain unchanged.
- Pi-todo supplies outcome and subtask counts, active summary, and plain text. Wrangle composes these into its strip and noninteractive readout without calculating todo status rules.
- Create a fresh hosted component per inline expansion. Preserve it across ordinary redraws. Update it on branch changes. Dispose it on collapse, provider replacement, release, reload, or shutdown.

Validate the factory and returned component before accepting ownership. A successful presentation preflight must precede the claim, including when the panel is closed. Surface creation, update, and rendering failures. Release the display claim and restore the standalone widget without changing tasks.

Version mismatch leaves the standalone widget available and reports unavailable shared todos. Do not flatten hierarchical state for a version 1 consumer or maintain duplicate renderers for mixed versions. Release the two first-party packages together when publication is separately authorized.

Evidence: installed Pi 1.3.0 `docs/extensions.md` and `docs/tui.md` describe the event bus and composable component lifecycle. `examples/extensions/tools.ts` demonstrates a host forwarding render, invalidation, and input. `docs/packages.md` requires independent package roots without undeclared imports. The factory contract is new application code, not an existing Pi API.

### Task contract

Keep the current operations and extend their arguments. `items` accepts strings or `{ content, subtasks: string[] }` for root creation. Child creation uses `append` with `parent` and string items. `start`, `done`, and `drop` use exact `task` content plus optional `parent`.

```json
{
  "op": "init",
  "items": [
    {
      "content": "Password reset",
      "subtasks": [
        "Implement reset tokens and behavior tests",
        "Implement reset form and interaction tests"
      ]
    },
    "Update operational documentation"
  ]
}
```

```json
{ "op": "start", "parent": "Password reset", "task": "Implement reset tokens and behavior tests" }
```

Root names must be unique. Child names must be unique within their parent. The same child label can exist under different parents. Reject grandchildren, blank labels, duplicate siblings, unknown parents, and malformed requests atomically.

Stored roots gain optional `subtasks`, with an empty default for existing snapshots. Child records contain content and status only. Rust remains the sole state validator. Do not persist display components or synthetic task IDs.

### Progress rules

- Create and append work as pending. `start` explicitly activates the target without demoting other active work. Stop automatically starting the next task.
- Starting a child also opens its parent. Starting a parent does not activate all its children. Starting closed work explicitly reopens it.
- Keep a parent open after its children close. Show `Ready for review` when all children are completed or abandoned. Derive that label, not a stored fifth status.
- Reject `done` for a parent with open children. Reject untargeted `done` if any parent has open children. The agent closes children before the parent.
- Dropping a parent abandons its open children but preserves completed and already-abandoned children. Untargeted close operations affect open work only.

Appending children to a closed parent reopens the parent as pending. Appending to an active parent preserves its status. All operations remain branch-local and atomic.

Existing snapshots preserve their recorded statuses on read. New operations use the parallel rules. Document the deliberate removal of automatic activation.

Show outcome progress separately from subtask progress. Completed counts never include abandoned items. A ready-for-review parent remains unfinished until explicit completion.

### Agent guidance

Creation responses and the tool description include a short instruction: size each executable unit for one agent session, with one deliverable and acceptance condition. Split work that spans independent outcomes or needs another session. Combine trivial steps with their deliverable and tests.

Delegate independent units separately. The lead can execute one unit. Do not delegate an oversized parent as one assignment. Keep dependent units ordered. These are prompts, not dispatch gates or numerical size limits.

Reuse this wording in approved-plan continuation. Completion reminders name open children and parents awaiting integration review. Preserve reminder limits and the existing progress guard.

## Files to modify

Paths are relative to `/Users/oisin/dev/oisinpi` unless explicitly rooted elsewhere:

- `crates/todo-core/src/{lib,operations,widget,reminder}.rs` and `crates/todo-wasm/src/lib.rs`: hierarchy, progress, presentation projections, serialization, and adjacent core tests.
- `packages/pi-todo/src/{index,tool,wasm,widget,presentation,work,plannotator}.ts` and adjacent tests: tool contract, guidance, reusable rendering, and display handshake. Add a focused embedded-view module only if needed.
- `/Users/oisin/dev/wrangle/pi/{work-todo,work-panel,work,index}.ts` and adjacent tests: host the provider component, preserve panel controls, and remove duplicate todo rendering.
- `packages/pi-todo/README.md`, `.changeset/*.md`, `/Users/oisin/dev/wrangle/docs/pi-work.md`, and Wrangle README: document behavior and the versioned presentation contract.
- Collection dependency metadata and lockfiles only during a separately authorized release update. No installed package edits, deployment changes, or new dependencies.

## Reuse

- `init`, `normalize`, and `view` in `crates/todo-core/src/lib.rs` own task creation, active selection, and model output.
- `runTodo` in `packages/pi-todo/src/tool.ts` applies operations and persists only successful changes.
- `latestSnapshot` and `STATE_ENTRY` in `packages/pi-todo/src/state.ts` preserve branch-backed state.
- `TodoPresentation` and `TodoWork` own the standalone/shared-display handshake. Extend their claim, release, and shutdown paths.
- `TodoWidget`, native `Text`/`ScrollView`, Wrangle `WorkPanel`, and `pi/work-contract.test.ts` provide rendering, keyboard, viewport, and real-provider test seams.

## Steps

- [x] Confirm guidance-only delegation, two levels, parallel activity, lead participation, shared rendering ownership, and explicit parent review.
- [ ] **Unit 1: one todo presentation owner.** Implement the version 2 provider and real Wrangle host together, initially against flat tasks. Own pi-todo widget/presentation/work modules, Wrangle work UI modules, contract tests, and contract documentation. Acceptance: `PI_TODO_SOURCE=/Users/oisin/dev/oisinpi/packages/pi-todo/src npm test && npm run typecheck` in Wrangle, plus `moon run pi-todo:test pi-todo:lint` in oisinpi. Prove reuse, both load orders, updates, disposal, mismatches, and standalone restoration. Generated output: existing gitignored WASM bindings only.
- [ ] **Unit 2: bounded hierarchical work.** After Unit 1 merges, implement the Rust hierarchy and transitions with tool schema, guidance, real WASM callers, shared-renderer updates, reminders, and Plannotator handoff. Own the listed todo core/WASM/TypeScript files, tests, README, and changeset. Acceptance: `moon run todo-core:test todo-wasm:test pi-todo:test pi-todo:lint`, plus the affected real provider/consumer contract test. Generated output: existing WASM bindings and normal Rust build outputs. No hand-edited generated files.
- [ ] **Integration acceptance.** Inspect affected targets, run the selected final checks once, and exercise both source extensions in an isolated Pi session. Inspect the scoped diff in each repository. Preserve unrelated work.
- [ ] **Release handoff.** Record compatible package versions and explicit remaining publication/install steps. Do not publish, install, update collection pins, or change managed configuration without the applicable approval.

Execution note: initiative capture returned HTTP 500 because the memory service has no LLM provider. This approved plan and native todos retain the task state.

The units are dependent, not parallel writer assignments. Keep each cross-repository display contract in one unit. The lead owns one unit at a time. Delegate independent read-only review when implementation is ready. Capture the initiative after plan approval and before code changes.

Both read-only planning investigations are complete. The first reader used stale remote source, so its conflicting claims were rejected. The second reader verified the current local embedding boundary. No implementation or acceptance checks ran.

## Verification

Run no tests or builds during planning. Before implementation checks, inspect `moon query tasks --affected` and each repository's current hook configuration. Run one check at a time. Use scoped fixers once after each unit, and avoid duplicate hook or unchanged-revision checks.

- **State and operations:** mixed standalone/nested creation, sibling duplicates, cross-parent labels, rejected grandchildren, exact addressing, multiple active units, explicit starts, reopen, parent review, drop, bulk close, and failure without persistence.
- **Persistence and callers:** old flat snapshots, malformed snapshots, resume/tree navigation, nested codemode calls, approved-plan replay without duplicate work, and no reset of unrelated progress.
- **Guidance and reminders:** creation response wording, tool schema descriptions, Plannotator continuation, open-child and review reminders, unchanged three-reminder budget, and unchanged progress guard.
- **Hosted rendering:** real provider/consumer tests in both load orders, selection and scroll preservation, no duplicate todo rows, branch updates, stale revisions, mismatched versions, render/update exceptions, idempotent disposal, and standalone restoration. The integration test must not silently skip because `PI_TODO_SOURCE` is unset.
- **Interactive acceptance:** load both source extensions without installed duplicates. Exercise regular/fullscreen modes, narrow/wide widths, long and wide-character labels, theme invalidation, inline expansion, Alt+J/K scrolling, Alt+I detail/back, and Alt+W collapse with an unchanged editor draft. Use the actual todo tool to start two siblings, finish them, inspect the ready-for-review parent, and complete it explicitly. Check `/todos`, Alt+T, `/todos list`, and All-to-Todos navigation. End all test-owned UI resources afterward.

Run Rust clippy for affected crates at the final revision, plus only affected package tests and lint/type checks. Pi UI-only changes do not justify Wrangle execution-core builds. Follow the configured gates rather than adding a second verification pipeline.

Generated outputs are existing gitignored WASM bindings in `packages/pi-todo/src/generated/` and normal Rust build outputs. No generated files are edited by hand.
