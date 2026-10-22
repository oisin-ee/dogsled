# `unit settlements` checks only the caller's open units

**Outcome.** `dogsled unit settlements` contacts hosts only for the caller's units that still wait for settlement, so it returns in well under a second on a ledger with many settled or foreign units. The service still refreshes every owner's open units.

**Cause.** `refresh` (`src/unit_request.rs:437-512`) walks every committed unit admission for every owner, including units whose settlement the owner already acknowledged. For each remote one it runs a sequential `ssh … host unit-receipt` (lines 456-463). `settlements` (line 676) and `UnitCommand::Observe` (lines 758-760) call it on the owner's request path. Pi's extension awaits `unit settlements` in `UnitBridge.drain` (`pi/unit-bridge.ts`, ~line 150) at session start, in `before_agent_start`, and in `agent_settled`. Measured on the user's machine: 48 committed unit admissions (32 remote, 28 already acknowledged), `unit settlements` takes 22.2 s, and Pi's first prompt waits ~40 s before the `input` event fires.

**Design.**
- Give `refresh` a scope: `pub fn refresh(fleet: &Fleet, scope: Scope<'_>)` with `enum Scope<'a> { All, Owner(&'a str), Execution(&'a ExecutionRef) }`. Names are yours; keep it one function.
- In every scope, skip a unit whose `settlement_ack.is_some()`. The owner acknowledges only after `settlements` ran `refresh`, which replays the item reply first, so nothing is left to repair.
- Keep re-checking host evidence for unacknowledged units, as today. Existing tests depend on it, for example `invalid_host_evidence_never_falls_back_to_cached_return_projection` (`tests/cli/unit_seeded.rs:396`). Do not import a saved `terminal_receipt` instead of asking the host.
- `Owner(o)` keeps units with `unit.execution.owner_id == o`. `Execution(e)` keeps only `unit.execution == *e`.
- Callers: `settlements` computes `owner()?` first and passes `Owner`; `Observe` decodes its input first and passes `Execution(&input.execution)`; `sweep::children` (`src/sweep.rs:210`) and `inbox::tick` (`src/inbox.rs:965`) pass `All`. `settle_unlaunched` stays unscoped (local only, no SSH).
- No new remote command and no per-host batching in this unit.

**Edits.**
1. `src/unit_request.rs`: scope parameter, the ack skip, the two internal callers.
2. `src/sweep.rs`, `src/inbox.rs`: pass `All`.
3. `tests/cli/unit_seeded.rs`: new tests. Use the `ssh` script and `ssh.log` pattern from `selected_remote_host_receives_exact_seed_and_durable_receipt_refresh` (line 1025) and the service path from `existing_service_refresh_imports_receipt_and_does_not_close_unsealed_unit` (line 1092):
   - owner A's `unit settlements` makes no SSH call for owner B's open remote unit, and none for A's own acknowledged remote unit, while still returning A's pending event;
   - `unit observe` for one execution makes no SSH call for other remote units;
   - the service refresh still imports owner B's remote receipt.
4. `CHANGELOG.md` under `## Unreleased`, Fixed: `unit settlements` and `unit observe` no longer query every host for every owner's units; Pi no longer waits on them at startup and on each prompt.

**Owned paths.** `src/unit_request.rs`, `src/sweep.rs`, `src/inbox.rs`, `tests/cli/unit_seeded.rs`, `CHANGELOG.md`.

**Acceptance.** `cargo test --locked --test cli unit_seeded` passes, including the new tests.

**Checks.** `cargo test --locked --test cli unit_seeded`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`. The lefthook pre-commit hook runs; do not bypass it.

**Base.** `origin/main` at `4e2be386b8926a40afd4e585918a15339cf6151d`. Branch `fix/unit-settlements-scope`. Commit, push, open a draft PR. No merge, release, or install.
