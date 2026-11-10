# T2b: fix the three review findings on todo deltas

**Outcome:** `fix/todo-state-deltas` gets one more commit on `997f4f941b646d8895cb357decc15cddf0601f57` that fixes the three findings below. The reviewer rejected `997f4f9` with one P1 and two P2 findings on 10 November 2026.

## P1: an older extension can erase delta-only state

**Finding.** An older pi-dogsled reads only the last full `todo-state`. It then appends a full snapshot built from that stale list, and `pi/todo/src/branch-state.ts:105–108` correctly treats a full snapshot as a replacement. Every delta in between is lost, including execution bindings. The reviewer reproduced a second claim of the same unit after such a write.

**Decision (lead):** older extensions fail closed on a delta session. They must not read it or write to it. Do not try to merge a stale writer's snapshot.

**Mechanism.** The old reader validates every `todo-state` entry with the core parser.
- `crates/todo-core/src/deserialization.rs:28–36` rejects any top-level key except `tasks`.
- `crates/todo-wasm/src/lib.rs:33–38` runs that parser.
- In the base code, `branchSnapshot` parses every `todo-state` entry on the branch (`git show a479b93:pi/todo/src/state.ts`).

So:
1. **Fence:** before the first `todo-delta` on a branch, the new writer appends a fence `todo-state` entry. Its data has `tasks` and one extra top-level key, for example `{"tasks":[...current list...],"format":"todo-delta/1"}`. The old parser rejects it. The new reader treats it as a full snapshot of `tasks`, so a fence carries the current list and is not a no-op.
2. **Every later full snapshot carries the key:** once a branch is fenced, every full `todo-state` that the new writer appends (the size fallback) carries the same key. An old `latestSnapshot`-only reader then also fails, not just `branchSnapshot`.
3. **Old snapshot after the fence:** the new reader must reject a plain `todo-state` (no key) that follows a fence on the same branch. Only an old writer can produce one. Fail with `INVALID_TODO_DELTA: unfenced snapshot after delta history`; never treat it as a replacement. If the old-reader proof below shows that no old path can write after a fence, keep the check anyway as defence, and say so.
4. **No fence on a branch:** branches with no fence (no deltas yet) keep plain `todo-state` entries, so old sessions stay readable by both versions.

**Old-reader proof.** The retained reference fixture is not enough.
- Load the base extension's actual readers from `git show a479b93:pi/todo/src/*.ts` into a test fixture: `state.ts`, `reminder.ts`, the provider claim and import paths, and the tool. Build them against the same generated bindings.
- On a fenced branch, prove that each one throws, or returns its existing failure, before any append. Prove that no old path appends.
- If an old path appends anyway, stop and report it with file:line.

Update `pi/todo/README.md` State: older extensions refuse a session after its first delta, and the user must use the updated extension. Remove the line that says they see the last full snapshot.

## P2: deferred validation makes readers disagree

**Finding.** `branch-state.ts:148–164` applies changed tasks before validating them. Validation happens only in `materialize` (`list ??= ...`). So a malformed intermediate task (status `"lost"`) followed by a valid replacement passes `latestSnapshot` and `branchSnapshot`, but fails `reminderFacts`.

**Fix.** Validate every changed task, and every order change, when its entry is read. Reject the whole branch at the first invalid entry, for every reader, with the same error. Validating one task must cost O(task), not O(list). Use the core's task-level validation if it exists. Otherwise validate the changed task inside a one-task list with `parse`, and state whether that is equivalent to whole-list validation for status and unit fields. Cross-task rules such as unique IDs and the size check stay at materialize. List any rule that can only be checked on the whole list.

## P2: reminder replay copies every historical list

**Finding.** `pi/todo/src/reminder.ts:25` requests and keeps every frame's list. Each request clones and re-parses all tasks (`branch-state.ts:44,54–59`). The reviewer measured 1,110 clones for 10 one-task deltas over 100 tasks.

**Fix.** Rewrite the reminder facts so that they need only each frame's changed records, plus the list at the end or at the few points the facts need. Do not use every frame's list. Add a counting test: with N one-task deltas over M tasks, total clones are O(N + M). Assert a concrete bound at N=20, M=100.

## Tests

Use Vitest in the package's style:
- P1 fence and old-reader proof;
- the unfenced-after-fence rejection;
- every reader rejecting the malformed-intermediate branch identically;
- the reminder clone-count bound.

Keep all existing tests passing.

## Acceptance

- `(cd pi/todo && pnpm exec vitest run)` passes.
- `(cd pi/todo && pnpm exec oxlint --config ../../.oxlintrc.json --deny-warnings src)` is clean.
- `pnpm run typecheck` passes.
- `node --test pi/unit-contract.test.ts pi/work-contract.test.ts` passes.

Do not run `moon run pi-todo:*` except through the commit hook.

## Constraints

- **Owned paths:** `pi/todo/src`, `pi/todo/README.md`, `pi/unit-contract.test.ts`. Do not touch `crates/`.
- **Bindings:** generate them with `moon run pi-todo:generate` in your worktree first. The crates are unchanged at base.
- **Commit hook:** runs as configured, with no bypass. If the hook runs past your tool's 600 s limit, run the commit as a Dogsled job and read its output. If it fails on a test you did not touch, stop and report.
- **Commit message:** `fix(todo): fence delta sessions from older extensions and validate deltas on read`. Push `fix/todo-state-deltas`. Do not open a PR, merge, release, or install.
- **Base:** `997f4f941b646d8895cb357decc15cddf0601f57`. Check `git rev-parse HEAD` first.

## Reply

- head SHA and changed paths;
- for each old reader path, what it does on a fenced branch, with file:line in the base code;
- each acceptance command with its result;
- the reminder clone counts before and after;
- any validation rule that stays whole-list only.
