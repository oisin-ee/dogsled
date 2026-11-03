# Unit 1b — todo headline + description

Unit: todo tasks and subtasks get an optional `description` beside `content`. `content` becomes a short headline; `description` holds the detail. The dogsled sidebar shows the current todo as a bold headline with its description underneath, instead of one long sentence.

Repository and branch: oisin-ee/oisinpi, new branch `feat/todo-labels`.
Base: `origin/feat/todo-sections` at e3a7ef4189b18db34e52ed834d1973068a64cc0e (PR #30, unmerged; CI green). Open your draft PR **with base `feat/todo-sections`**, not main.
Prerequisites: unit 1 (typed `sections()`, protocol v3) — on your base.

## Change

1. **Core model** (`crates/todo-core/src/lib.rs`, `deserialization.rs`): `Task` and `Subtask` gain `description: Option<String>`, `#[serde(default, skip_serializing_if = "Option::is_none")]`. Add `"description"` to the hand-written field visitors and their expected-field lists. Saved branch state without the field must still load (test it).
2. **Tool input** (`packages/pi-todo/src/schema.ts`, core `Request`/operations): every place that accepts `content` for a new outcome or leaf (`init`/`append` items, outcome objects, subtask drafts; string shorthand stays content-only) accepts optional `description: string`. `update` may set or clear `description` on an open task or subtask; it never changes `content`-based selection. Empty or whitespace-only description normalises to none.
3. **Guidance** (`packages/pi-todo/src/guidance.ts` and the tool description): `content` is a short headline (aim under 60 characters, imperative, no trailing detail); put context, approach, or acceptance detail in `description` (one or two sentences). Guidance only — do not reject long content.
4. **Sections** (`crates/todo-core/src/widget.rs`): `SectionItem` and `SectionChild` gain `description: Option<String>`, copied from the task/subtask. Order, counts, and `widget()` output stay unchanged (the Pi inline widget keeps headlines only).
5. **Model-facing text** (`view()`): show a task's description on the line after its headline, indented two spaces beyond the headline, so the model keeps the detail it wrote. Update the affected snapshot tests deliberately.
6. Wasm/TS types regenerate from tsify; `display.sections` carries the new field with no further TS changes beyond types. Protocol stays version 3 (additive field; dogsled's unit 3 is built against it).
7. Docs: `packages/pi-todo/README.md`, `docs/todo-workflows.md` — the new field and the headline guidance. Add to the existing `.changeset/todo-sections.md` text (same release).

## Scope

Owned paths: `crates/todo-core/src/` (lib.rs, deserialization.rs, operations.rs, widget.rs, the view/summary module, and their tests), `crates/todo-wasm/src/lib.rs` (only if a signature needs it), `packages/pi-todo/src/` (schema.ts, guidance.ts, tool.ts, tests/fixtures), `packages/pi-todo/README.md`, `docs/todo-workflows.md`, `.changeset/todo-sections.md`.
Generated paths: `packages/pi-todo/src/generated/` via `moon run pi-todo:generate` (gitignored; do not commit).
Read first: `AGENTS.md`, `crates/todo-core/src/lib.rs` (`Task`, `Subtask`), `deserialization.rs`, `operations.rs` (init/append/update), `widget.rs` (`sections`), `packages/pi-todo/src/schema.ts`, `guidance.ts`, `tool.test.ts`.

## Checks

Acceptance (you run it): `cargo test -p todo-core && cargo clippy -p todo-core -p todo-wasm --all-targets -- -D warnings && moon run pi-todo:test pi-todo:lint` — all pass, with new tests:
- init/append with `description` stores it on task and subtask; string shorthand has none;
- update sets and clears a description; whitespace normalises to none;
- old saved state without `description` loads unchanged;
- `sections()` carries `description`; `widget()` output unchanged;
- `view()` prints the description under its headline.

Note: the workspace denies `clippy::too_many_lines` (60) — tests included. The pre-commit hook runs on commit; do not bypass it.
Forbidden heavy checks: `moon ci`, whole-workspace builds, installs beyond worktree setup.

## Publication authority

Commit: yes. Push `feat/todo-labels`: yes. Draft PR with base `feat/todo-sections`: yes. Merge, release, publish: no.

Failure behaviour: if `update` semantics or legacy exact-content selectors conflict with editing `description`, stop and report the evidence; do not change selector behaviour.

## Reply

Branch, head SHA, draft PR URL, changed paths, commands with results, unverified scope.
