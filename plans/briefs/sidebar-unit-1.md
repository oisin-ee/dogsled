# Unit 1 — pi-todo typed todo sections (work protocol v3)

Unit: u-9fbe63a3-6003-4d0f-b39d-8cb412c11b91-10. pi-todo publishes its status groups as typed data (`display.sections`) so dogsled's new sidebar can lay them out. Today they only exist as flattened strings (`WidgetRow { kind, text: "In progress · 1" }`).

Repository and branch: oisin-ee/oisinpi, branch `feat/todo-sections`.
Base SHA: b2633f34ac1aba1d633566386405f7553b4251c9 (origin/main).
Prerequisites: none.

## Why

Dogsled is replacing its text side board with a ratatui sidebar (approved design: framed panels). The Todos panel shows a progress bar (done of total), a NOW block (in-progress tasks, wrapped), a NEXT block (pending), and one folded `✓ N done  − N dropped` line that expands in place. It cannot be built from pre-rendered ANSI lines. pi-todo must stay the only owner of task interpretation and grouping; dogsled only draws this projection.

## Contract (fixed; dogsled unit 3 is being built against it in parallel)

Rust (`crates/todo-core/src/widget.rs`, serde + tsify like `WidgetRow`):

```rust
pub struct TodoSection { pub status: Status, pub count: usize, pub items: Vec<SectionItem> }
pub struct SectionItem {
    pub status: Status,
    pub text: String,          // task.content
    pub review: bool,          // task.ready_for_review()
    pub unit: Option<String>,  // unit.summary()
    pub children: Vec<SectionChild>,
}
pub struct SectionChild { pub status: Status, pub text: String, pub unit: Option<String> }
pub fn sections(list: &TodoList) -> Vec<TodoSection>;
```

- Always exactly four sections, in this order: `in_progress`, `pending`, `completed`, `abandoned` (Status's existing snake_case serde names). `count == items.len()`; empty sections are present with `count: 0, items: []`.
- Items keep list order. `children` are all subtasks (not filtered by expansion).
- `widget(list, expanded)` is rebuilt on top of `sections()` so grouping lives in one function. Its current output and tests stay byte-identical.

Wasm (`crates/todo-wasm/src/lib.rs`): export `sections(list) -> TodoSection[]` beside `widget`.

TypeScript (`packages/pi-todo/src/display.ts`, `wasm.ts` re-exports): `TodoDisplay` gains `readonly sections: readonly TodoSection[]`, computed from the same `list` as `text`/`summary`.

Protocol (`packages/pi-todo/src/work.ts`): `protocolVersion` becomes `3`. v3 = v2 envelope + `display.sections`. Do not keep answering v2 requests: an incompatible version already falls back to standalone todos (documented behaviour), and dogsled moves to v3 in unit 3. Update the protocol doc wherever pi-todo documents it (grep `oisin.todo.snapshot` under `packages/pi-todo` and `docs/`).

Changeset: minor bump for `@oisin-ee/pi-todo` (and the `oisinpi` bundle if the repo's changeset config requires it), describing protocol v3 and `display.sections`.

## Scope

Owned paths: `crates/todo-core/src/widget.rs`, `crates/todo-core/src/lib.rs` (re-exports only), `crates/todo-wasm/src/lib.rs`, `packages/pi-todo/src/` (display.ts, wasm.ts, work.ts and their tests/fixtures), `packages/pi-todo/README.md`, `docs/` (protocol text only), `.changeset/`.
Generated paths: `packages/pi-todo/src/generated/` via `moon run pi-todo:generate` (gitignored; do not commit).
Read first: repository `AGENTS.md`/`README.md`, `crates/todo-core/src/widget.rs` (`group`, `widget`, tests), `packages/pi-todo/src/display.ts`, `work.ts`, `widget.test.ts`, the existing work-protocol tests.

## Checks

Acceptance (you run it): in the worktree, `cargo test -p todo-core && moon run pi-todo:test` — all pass, including new tests:
- `sections()` on a mixed list: four sections in order, counts, `review`, `unit`, children.
- `sections()` on an empty list: four empty sections.
- `widget()` existing tests unchanged and passing.
- a work-protocol test: a v3 request yields a snapshot whose `display.sections` matches `sections(list)`; a v2 request is rejected the same way other incompatible versions are.

Allowed light checks: `cargo test -p todo-core`, `cargo clippy -p todo-core -p todo-wasm -- -D warnings`, `moon run pi-todo:test`, `moon run pi-todo:lint`, `oxfmt --check` on changed files. The pre-commit hook runs on commit; do not bypass it.
Forbidden heavy checks: `moon ci`, whole-workspace builds/tests, installs beyond the worktree setup.

## Publication authority

Commit: yes. Push `feat/todo-sections` to origin: yes. Draft PR: yes. Merge, release, npm publish, version bumps outside the changeset: no.

Failure behaviour: if the contract above cannot be met as written (for example tsify cannot express a type), stop and report the exact blocker; do not change the contract shape on your own.

## Reply

Branch, head SHA, draft PR URL, changed paths, commands run with results, and anything unverified.
