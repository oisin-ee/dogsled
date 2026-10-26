# J1 repair 2: receipt correlation across prune, and bounded race-test teardown

## Outcome
A matching host-launch replay still succeeds after existing prune removes the terminal Job record. The real-wrapper race tests cannot hang or leave processes.

## Revision and scope
Repository: `/Users/oisin/dev/dogsled`. Reuse `fix/lifecycle-job-claims` WITHOUT a base argument. Retained head `7feffe978b515ae4a9f62e85fed2ae0617bcfd2e`. Preserve all commits; no reset, rebase or recreate.
Prerequisite: independent review `u-30fae40b-92bb-4491-a040-e16ce5bafc25-67` returned BLOCK at that exact SHA. It confirmed the four earlier findings are repaired and lock order is correct. The lead verified both new findings in source.
Owned: `src/work.rs`, `src/work_tests.rs`, `tests/cli/launch_retry.rs`, `tests/cli/stop_sweep.rs` (one prune-then-replay regression only, if it fits there better), `tests/cli.rs` (shared rig helpers only).
Read: `src/jobs.rs`, `src/cleanup.rs` (`prune_only` ~872, `remove_job` ~1892), `src/ledger.rs`, prior briefs `plans/briefs/lifecycle-job-claims{,-repair,-repair-review}.md`.
Do not change `src/cleanup.rs` or prune policy. Job-metadata expiry (C8) and request age (J2) are separate units.

## Finding 1 (P2): matching replay breaks after existing prune
`work::correlate` (`src/work.rs:330–335`) requires `jobs::record`. Existing prune removes terminal `jobs/ID.json` after one day (`src/cleanup.rs:873–886,1892–1907`) but keeps `launches/ID.json`. A matching replay then refuses for ever.
Do NOT fix this by deleting receipts at prune. Without receipt and record, a replay would look unclaimed and dispatch again.
Required repair: make the saved job receipt self-sufficient correlation evidence. On a successful dispatch, record in the receipt the fields `correlate` compares: id, host, pane, command, cwd, owner, timeout, idle timeout. Keep the existing `id`/`host`/`pane` result keys unchanged for callers.
- Replay compares the request against the receipt's own fields.
- When the Job record is present, it must also agree (identity, pane, immutable fields), as now.
- When the Job record is absent, a complete matching receipt replays without side effects.
- A legacy receipt without the correlation fields replays only when the Job record exists and agrees. With neither, it refuses with a clear message and makes no allocation, dispatch, or receipt rewrite. Fail closed; do not guess.
- A receipt that conflicts with the request or record still refuses.

## Finding 2 (P2): race tests can hang and strand processes
`tests/cli/launch_retry.rs` ~1311 backgrounds the real wrapper without a handle. FIFO opens and reads have no deadline. The `wrapper-closed` marker proves only that the close branch ran, not that the wrapper exited. A regression or failed assertion leaves the wrapper and its FIFO-blocked command running, or blocks the test for ever.
Required repair, for every test that uses `dispatch_order_herdr` or FIFOs this branch added:
- The test owns every process it starts, including the backgrounded wrapper (for example via a recorded PID), and kills and reaps them on every exit path, including assertion failure (a drop guard or equivalent).
- Every barrier wait has a bounded deadline and fails with a diagnostic instead of hanging. Opening a FIFO counts as a wait.
- Wait for actual wrapper exit, not just a marker written from the close branch.
- Keep the forced event orders and real `dogsled exec`. No sleeps, test-only production flags, new dependencies, or detached leftovers.

## Required regressions
- Prune-then-replay: a job dispatches successfully, ends, and its Job record is removed by the real prune path (or exactly what `remove_job` removes). An identical replay returns the saved result; logs show no `tab create` or `pane run`. Prove red at `7feffe9`.
- Same setup with a conflicting request refuses, with no side effects.
- Legacy receipt (only `id`/`host`/`pane`) with no Job record refuses without side effects. With a matching Job record, it replays.
- Bounded teardown: show the race tests now finish and leave no wrapper process when the barrier is never released (run once deliberately broken, or reason from the bound). Report how you proved it; no committed flaky test.
Keep all current J1 regressions and the `interrupted_job_admission_preserves_running_record_without_relaunch` negative control unchanged.

## Acceptance and check budget
In the linked worktree run:
`cargo test --test cli jobs:: && cargo test --test cli lifecycle:: && cargo test --test cli launch_retry:: && cargo test --test cli review::closed_wrapper_stdout_preserves_full_log_and_child_exit && cargo test --bin dogsled work_tests::`
Add `cargo test --test cli stop_sweep::` if you edit that file. Run the required pre-commit hook (`mise run check:rust`); never bypass it, and do not repeat the full suite outside it. Preserve exit status when piping. If a test run times out, find and kill only the processes your test started, then report it.
Report red-before and green-after commands with exit codes, the exact final SHA, and changed paths.

## Authority and handoff
Scoped LOCAL commits on the retained branch only. NO push, PR creation or update (PR #155 is unauthorized; leave it), merge, release, deployment, live cleanup, dependency or unrelated edits.
If the receipt change needs a path you do not own, stop and report the exact blocker. Use the seeded unit and native done.
