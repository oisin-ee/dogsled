# Review J1 repair 3 before integration

## Outcome
PASS or BLOCK for integrating J1 (job claims) into local `main`, judged at an exact revision. Read-only.

## Revisions
Repository: `/Users/oisin/dev/dogsled`. Review **`4dbbe3de5950d29837f4bfd146c7eb56fc23a0b1..a6970b2b1e34ce9174c88109a370cb51c588f268`** (branch `fix/lifecycle-job-claims-r2`). Focus on the new commit `599bfde..a6970b2` (5 files, +363/−48). `git merge-tree` against local `main` is clean.
Contract: `plans/briefs/lifecycle-job-claims-repair-3.md`. Previous review: `plans/briefs/lifecycle-job-claims-r2-review.md` and its three findings, quoted in the repair-3 brief.

## The three findings this commit must close
1. **Complete receipt.** The recordless branch of `correlate` (`src/work.rs`) must refuse a receipt whose `pane` is missing, null, empty, or not a string, before it returns `Ok`. Check that `id` and `host` are also checked before that return; they were checked earlier in the function at `599bfde`. Confirm this.
2. **Owned race-test processes.** Every spawned process keeps its `Child` handle. The command's own process group (`src/jobs.rs:207`) is tracked and killed on failure. In the success path, the test proves that the wrapper exited within a bound **before** any fallback kill. Check `tests/cli.rs` (`Owned`, `wait_gone`, `process_gone`, `read_fifo_within`, `Fifo::hold`) and `tests/cli/launch_retry.rs`. Check that each wait has a bound, and that no comment claims a group covers processes it does not.
3. **FIFO signal.** No writer is dropped before the reader has acknowledged. Check every FIFO use in the owned tests.

## Scope deviation to judge
The brief said **not** to edit `tests/cli/review.rs`. The writer changed it anyway (+20/−19): the closed-wrapper test now uses `Owned::spawn` and a non-blocking `open_fifo`. Judge whether to keep it:
- Is it test-only and correct?
- Is `owned.wait()` bounded?
- Its comment says a failed assertion kills the wrapper "and every command it started". Is that true, given that the command runs in its own process group?
- Does the writer stay open until the exit?
Say whether keeping it is fine, or whether it must be fixed or reverted before integration.

## Also check
- The four earlier fixes and the prune/replay lifetime fix stay intact. The lock order is unchanged.
- The writer's report is untrusted:
  - red before at `599bfde`: 2 regressions;
  - green after: jobs 14, lifecycle 6, launch_retry 48, stop_sweep 12, review 1, work_tests 9;
  - full suite 319/296/6, race tests run 5 times, hook passed.
  Do not rerun. Read the tests and judge whether they prove the claims.

## Reply
PASS or BLOCK. For each finding give severity, file:line at `a6970b2`, and the smallest fix.

## Limits
Read-only. No edits, test runs, builds, publication, or cleanup. Use the seeded unit and native done.
