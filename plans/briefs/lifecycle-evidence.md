# L1E: one owner for agent lifecycle state

## Outcome
One function derives each agent's lifecycle state from positive evidence. It sits in a new `src/lifecycle.rs`, and the host snapshot publishes its result. Raw Herdr terminal states stay available, unchanged, for the capacity and safety guards. This slice adds no new board groups and does not produce `ask` or `blocked` signals; those come in later units.

## Revisions
- Repository: `/Users/oisin/dev/dogsled`.
- Base: **the local `main` revision after J2 (request validity) is integrated.** The lead fills in the exact SHA at dispatch.
- Work on a new branch, `feat/lifecycle-evidence`. Local commits only.

## Contract
Read `plans/lifecycle-cleanup.md`, section 1, "Agent lifecycle". It defines the states and their signals:
`queued`, `starting`, `working`, `waiting`, `asking`, `blocked`, `finishing`, `done`, `stopped`, `lost`, `unknown`.

Rules this slice must keep:
- **Silence never means completion.** Missing, unreadable or contradictory evidence gives `unknown`. It never gives `stopped` or `done`.
- **Missing agent data alone never means `starting`.** `starting` needs correlated admission or launch progress.
- **`done` is reached only through native `done`.** It needs a native `done`, a sealed final reply and the required terminal evidence. Herdr's `done` means only that the turn ended. Normalize it inside the derivation and nowhere else.
- **`finishing`** means `done` was accepted but the handoff is not yet complete.
- **A recorded final handoff stays `done`**, even when its host later becomes unavailable.
- **`lost`** needs host-freshness evidence, or identity-correlated proof that the pane is gone, without a final handoff.
- **`waiting`** uses the same ownership scan as the done guard: the agent's open items, plus its pending or admitting child tickets.
- **Explicit `asking` and `blocked`** outrank inferred `waiting`. In this slice, `blocked` comes only from Herdr `blocked`, which means an approval dialog is open. The derivation must still accept typed `ask` and `blocked` inputs, as an optional field that is always `None` for now.
- **Keep three things separate:** agent state, resource closure and result. A closure blocker or a result such as "ready to merge" must not change the state.

## Required
1. Add `src/lifecycle.rs` with one pure derivation over an evidence struct, and table tests for every state and every precedence rule above. That includes the unknown cases and the case where a lost host has a recorded done.
2. Publish the derived state and its evidence gap in the host snapshot, beside `done::observe` (`src/done.rs`, `src/node.rs`, `src/probe.rs`, `src/hosts.rs`). Older readers must still decode the snapshot.
3. Point the existing call sites in `src/board.rs`, `src/items.rs` and `src/launch.rs` at the new owner, wherever they read state today. Do not change board groups or wording; that is unit `-13`.
4. `src/work.rs`: only the minimal launch-progress evidence the derivation needs. Do not touch the J1 or J2 claim and validity code.

## Owned paths
- `src/lifecycle.rs` (new)
- `src/main.rs`, `src/probe.rs`, `src/node.rs`, `src/hosts.rs`, `src/items.rs`, `src/done.rs`, `src/done/tests.rs`, `src/launch.rs`, `src/work.rs`, `src/board.rs`
- `tests/cli/done.rs`, `tests/cli/context.rs`

Do not edit any other file.

## Acceptance
```sh
cargo test --bin dogsled lifecycle:: && cargo test --bin dogsled board:: && cargo test --test cli done:: && cargo test --test cli context::
```
- Run it once and record its exit code.
- The pre-commit hook (`mise run check:rust`) must pass at the final commit. Do not bypass it.
- Report the exact SHA.

## Limits
- Start at once with the first edit.
- Write code into files with edit tools, one small piece at a time. Keep your replies to a few lines; do not draft code or long reasoning in chat.
- If you make 20 tool calls without an edit, stop and report.
- If a rule above conflicts with the source, stop and report the conflict in one line. Do not invent behavior.
- Use the seeded unit and native done.
