# Brief: the lead closes its own tasks with a note

## Outcome

In pi-todo, the session that owns a task list can close any open leaf with `done` or `drop`. `done` needs only a short note. This also applies to a leaf that a worker holds. Closing such a leaf releases the worker's binding, so its owned paths stop blocking other units.

## Why

User decision. The current rule blocks the lead from closing finished work.

- `crates/todo-core/src/operations.rs` `completion()` (about line 395-418) refuses `done` with `ACCEPTANCE_REQUIRED: start local work or obtain released worker evidence before completion`. It refuses unless the leaf is the active local unit, or `provider::has_review_evidence` is true. That function needs a worker settlement.
- When a worker never settles, the leaf can never close. Its binding also keeps its paths held, and `validation.rs` (about line 250) then returns `SCOPE_CONFLICT` for any new unit that overlaps them.
- `operations.rs` (about line 512-517) passes every bound unit to `provider::protect`, so even `drop` of a bound leaf fails with `UNIT_ALREADY_BOUND`.

Live example: one stale worker binding held `README.md`, `CHANGELOG.md`, `src/cli.rs` and `src/main.rs` in the Dogsled lead's list for a day, and blocked a release. The worker's work had merged long before.

## Required behaviour

1. `done` on an open leaf needs a non-empty `evidence` string, and nothing else. Remove the local-session and settlement condition from `completion()`. Keep the existing rules for an explicit unit ID, open children, the contract check and prerequisites.
2. `done` or `drop` on a bound leaf in the owning list clears `binding` and sets the status. Record the released execution in a new `Unit` field, for example `released: Vec<ExecutionRef>`, with serde default so old files still load.
3. A later settlement whose execution is in `released` is a no-op: `settle` returns `applied: false` and changes nothing. Dogsled then acknowledges the event, and its pending queue drains. Today this case returns `BINDING_MISMATCH` and the event stays pending for ever.
4. The held-unit protection in `operations.rs` still refuses every other change to a bound unit. Allow only the explicit close of the selected leaf, or of the children of a dropped parent.
5. `pi-todo` in-flight call leases (`packages/pi-todo/src/provider.ts`, `protect(before, list, leases)`) are not changed. A leaf with a live spawn call still cannot close.
6. A seeded worker's own imported list keeps its current rules (`SEEDED_SCOPE_LOCKED`, seal on `done`).
7. Text: change `packages/pi-todo/src/guidance.ts` line 10 and the `todo` tool description in `packages/pi-todo/src/index.ts` so that `done` asks for a short note. Remove "worker exit alone is not acceptance" and the "Done requires acceptance evidence" wording. Update `plans/todo-workflows.md` lines 113-115 to match.

## Tests (Rust in `crates/todo-core`, TS in `packages/pi-todo`)

- The lead does `done` with a note on a leaf that is not the active local unit: the leaf completes.
- The lead does `done` on a bound worker leaf: the leaf completes, the binding is cleared, the execution is in `released`, and a new unit that overlaps its old paths can start.
- The lead does `drop` on a bound leaf, and on a parent with a bound child: both release.
- A `returned`, `stopped` or `not_started` settlement for a released execution gives `applied: false` and no change. A settlement for a different execution still fails.
- Any other change to a bound leaf, such as `update`, still fails with `UNIT_ALREADY_BOUND`.
- A leaf with a live call lease still cannot close.
- `done` without a note still fails.
- Change the existing `ACCEPTANCE_REQUIRED` expectations in `hierarchy.test.ts`, `tool.test.ts` and the Rust tests to the new rules. Do not delete coverage of the rules that remain.

## Owned paths

- `crates/todo-core/src/` (operations, provider, validation, contract, binding, and their tests)
- `crates/todo-wasm/src/lib.rs`, only when a signature change requires it
- `packages/pi-todo/src/` (guidance, index, provider, and tests)
- `plans/todo-workflows.md`
- `.changeset/todo-lead-close.md` (patch bump for `@oisin-ee/pi-todo`)

`packages/pi-todo/src/generated/` is gitignored. The `generate` moon task writes it. Do not commit it.

## Checks

- `cargo test -p todo-core`
- `proto run moon -- run pi-todo:test` (this builds the wasm and runs the package tests)
- The commit hooks

Do not run `moon ci`, e2e tests, a release, a publish or an install. Do not poll CI.

## Publication

Open one draft PR to `main`. Do not merge, version, or publish.

## Return

PR URL, head SHA, changed paths, the exact check commands and their results, and any rule you could not keep.
