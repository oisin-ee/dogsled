# Review J1 repair 2 before integration

## Outcome
PASS or BLOCK for integrating J1 (job claims) into local `main`, judged at an exact revision. Read-only.

## Revisions
Repository: `/Users/oisin/dev/dogsled`. Review **`4dbbe3de5950d29837f4bfd146c7eb56fc23a0b1..599bfde037f4716f7c3007a958f4dfa433167c58`** (branch `fix/lifecycle-job-claims-r2`). Focus on the new commit `7feffe9..599bfde` (5 files, +520/−58: `src/work.rs`, `src/work_tests.rs`, `tests/cli.rs`, `tests/cli/launch_retry.rs`, `tests/cli/stop_sweep.rs`). `git merge-tree` against local `main` is clean.
Contract: `plans/briefs/lifecycle-job-claims-repair-2.md`. Earlier findings: `plans/briefs/lifecycle-job-claims-repair-review.md`. Plan: `plans/lifecycle-cleanup.md` sections 6–7.

## The two findings this commit must close
1. **Receipt lifetime.** `correlate` required the Job record, but existing prune (`src/cleanup.rs:873–886,1892–1907`) deletes terminal Job records after one day and keeps receipts, so matching replay refused for ever. Check: the receipt now carries its own correlation evidence (`request`: cmd, cwd, owner, timeouts) together with id, host, and pane. A matching replay after prune returns the saved result with no new pane or dispatch. A conflicting receipt refuses. A legacy receipt with no evidence and no Job record fails closed. While a Job record survives, a legacy receipt is still checked against it.
2. **Bounded race tests.** The race tests must own, bound, and reap every process they start, including on failure, and must prove that the wrapper exited (not only a marker). Check the new helpers in `tests/cli.rs` (process-group guard, non-blocking FIFO, 20 s bound, `own_group()` check) and their use in `launch_retry.rs` and `stop_sweep.rs`.

## Also check
- Confirm that the four findings fixed at `7feffe9` stay fixed. The lock order and the J1 claim checks are unchanged.
- Any path that now accepts a receipt it should refuse. For example: compare which fields the receipt check covers with the fields the Job record check covers, and look at the host and pane checks when no record is present.
- The receipt's `request` object is also returned in the launch result. Check that no caller treats it as something it is not.
- Writer report (untrusted): red before at `7feffe9` (3 regressions failed); green after: jobs 14, lifecycle 6, launch_retry 45, review 1, work_tests 9. Full suite 319/293/6. The hook passed. Do not rerun the suite. Read the tests and judge whether they prove the claims.
- Known and out of scope here: `tests/cli/review.rs::closed_wrapper_stdout_preserves_full_log_and_child_exit` still uses a blocking FIFO open and an unbounded `wait()`. That pattern predates J1, which only added a `rig.claim` call (`git diff 4dbbe3d 599bfde -- tests/cli/review.rs`). Say whether it should block integration.

## Reply
PASS or BLOCK. For each finding give severity, file:line at `599bfde`, and the smallest fix.

## Limits
Read-only. No edits, test runs, builds, publication, or cleanup. Use the seeded unit and native done.
