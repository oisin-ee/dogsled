# J2 repair: close four gaps in the request-validity checks

## Outcome
Fix four defects that the independent review found at `e1c1062`. The lead confirmed each one in the source. Once they are fixed, no expired or unprovable job request can start new execution, change capacity, or release ownership, and the clock bootstrap can no longer move backward.

## Revisions
- Repository: `/Users/oisin/dev/dogsled`.
- Continue the retained branch `feat/lifecycle-request-validity` from `e1c1062d90bd7acb61a17df52d3c258c7befcc0a`. Add new commits on top; do not rewrite history.
- The contract is `plans/briefs/lifecycle-request-validity.md`. These four findings break its items 3 and 5. The general rule for a refusal: it may record its error and advance the clock, but it must not allocate, dispatch, release ownership, or write terminal evidence.

## Fix
1. **Wrapper: recheck before capture** (`src/jobs.rs:710–712` and `exec` at about `735–745`).
   - The blocking pane `report` runs between the startup check and `capture`, so a request can expire in that gap.
   - Recheck validity immediately before `capture`. If the recheck refuses, start no process. Leave the claim and any other ownership in place, and write no terminal outcome that claims the job ran. If the record needs a state, use one that is not terminal.
   - Make the comment accurate.
2. **Host launch: recheck after allocation** (`src/work.rs:591–600` and `launch_job`).
   - Recheck validity after pane allocation, or after any blocking preparation, and before the executable claim is published and the command is dispatched.
   - If the recheck refuses: do not dispatch, do not write the receipt, and do not call `prepare::used` (do not release preparation). Retire only the pane this attempt allocated, through the existing launch-pane cleanup path. Do not touch any other ownership.
3. **Reservation: validate first** (`src/node.rs:162–174`).
   - Check validity **before** `live_reservations` (that call rewrites capacity records) and before the early return for an existing reservation.
   - A refused request must not change `placements.json` and must not get `reserved: true`.
   - The existing-hold replay is not new admission. Keep returning it unchanged only when validity passes. If the hold exists and validity fails, refuse and leave the hold untouched.
4. **Bootstrap fails closed** (`src/jobs.rs:103–111`, `json_files`).
   - When the clock file is missing, the bootstrap must account for every local record. If a record directory exists but cannot be read, or a record file cannot be read or parsed, refuse with `job validity clock is unavailable; new execution refused`. Do not skip the record.
   - A record directory that does not exist is fine; it counts as empty.
   - Change only the bootstrap. Do not change how records are read anywhere else.

## Tests
Add four regressions in `tests/cli/request_validity.rs` or the unit tests. Each must fail at `e1c1062` and pass after the fix. Record both results.
- **Wrapper:** expiry crosses during the pane report, using the test clock or a fake pane report that advances it. Assert that no process starts and the claim remains.
- **Host launch:** expiry crosses during pane allocation. Assert that nothing is dispatched, there is no receipt, preparation is not released, and the allocated pane is retired.
- **Reservation:** an expired request with an existing hold, and one without. Assert that `placements.json` is byte-identical and the response is a refusal.
- **Bootstrap:** with the clock file missing and an unreadable or corrupt record present, execution refuses with the unavailable-clock message.

## Owned paths
- `src/jobs.rs`, `src/work.rs`, `src/node.rs`, `src/work_tests.rs`
- `tests/cli/request_validity.rs`, `tests/cli.rs`

Edit no other file.

## Acceptance
```sh
cargo test --test cli request_validity:: && cargo test --test cli jobs:: && cargo test --test cli launch_retry:: && cargo test --bin dogsled work_tests::
```
- Run the command once and record its exit code.
- The pre-commit hook (`mise run check:rust`) must pass at the final commit. Do not bypass it.
- Report the exact SHA.

## Limits
- Start immediately with the first edit.
- Write code into files with edit tools, one small piece at a time. Never draft code or long reasoning in chat. Keep replies to a few lines.
- After 20 tool calls without an edit, stop and report.
- Local commits only. Use the seeded unit and native done.
