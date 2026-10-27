# Review J2: recorded job request validity

## Outcome
Give PASS or BLOCK on merging J2 into local `main` at `e1c1062d90bd7acb61a17df52d3c258c7befcc0a`. This review is read-only.

## Scope
- Review only `de9d043..e1c1062`: one commit on branch `feat/lifecycle-request-validity`, 21 files, +1865/−195.
- `de9d043` is the J1 merge and has already been reviewed. Do not re-review J1.
- The contract is `plans/briefs/lifecycle-request-validity.md`, with its design in `plans/briefs/lifecycle-request-validity-design.md` and `plans/lifecycle-cleanup.md` section 7. The user selected these decisions:
  - A receipt replay after expiry is allowed and read-only.
  - A missing clock file is created. A corrupt or unwritable clock refuses.
  - A request may be up to 5 minutes ahead of the host clock.

## Check
1. **Validity is stamped exactly once.** It is set in `runtime::request` and copied unchanged into the claim and the receipt. A changed timestamp conflicts.
2. **All five refusal points refuse before any side effect.** The points are queue insertion, admission, host reservation, host launch, and wrapper start. Side effects include capacity, the claim, a pane, and the startup record. A refusal must not release unresolved ownership.
3. **The clock cannot be rolled back.**
   - `effective_now = max(high_water, wall)` is persisted before the result returns, including when the request is refused.
   - The leaf lock fits the lock order: admission → preparation-receipt → Git common-dir → done → launches → launch-panes → short store locks.
   - A missing, corrupt or unwritable clock behaves as the decisions above say.
4. **The boundaries are exact.** 30 days is valid; 30 days + 1 ms is expired. +5 minutes is allowed; +5 minutes + 1 ms is refused. The messages match the four refusal strings in the plan.
5. **Running jobs are left alone.** A running job is never killed because of its age, and a correlated replay stays read-only after expiry.
6. **Peer protocol.** The version flag is required on `host launch` and `reserve`, with no fallback. Agent launches now pass `--job-validity 1` too (`tests/cli/lifecycle.rs`, `close.rs`). Decide whether that is correct, or whether it needlessly breaks agent launches to older peers. Report it as **blocking** only if it contradicts the contract.
7. **Out-of-scope files.** These six files are outside the unit's owned paths: `src/context.rs`, `src/launch/pane_tests.rs`, `src/sweep.rs`, `tests/cli/close.rs`, `tests/cli/lifecycle.rs` and `tests/cli/placement.rs`. The lead read them, and they appear to be only fixture and signature updates forced by the API change. Confirm that they make no change to behavior.

## Blocking bar
- **BLOCK** only for a real defect in this change:
  - a path that executes without the validity check;
  - a side effect before a refusal;
  - a clock rollback;
  - a lost ownership hold;
  - a broken boundary;
  - a contract contradiction;
  - a weakened existing assertion.
- **Follow-up, not a block:** style, test-harness patterns, or concerns that existed before this change.

## Writer claims (untrusted; do not rerun)
- The seven acceptance commands are green, and the full suite passed 322/322/6.
- The hook passed.
- `review::command_deadline_bounds_host_lock_wait` failed once under load. This test is already a known follow-up: it does a blocking read at `tests/cli/review.rs:32`. It does not block this change.

## Reply
Give PASS or BLOCK first. Then list the findings. For each one, give its severity and `file:line` at `e1c1062`, and mark it **blocking** or **follow-up**.

## Limits
Read-only. Use the seeded unit and native done.
