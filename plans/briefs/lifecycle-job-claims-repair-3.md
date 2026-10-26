# J1 repair 3: complete receipt evidence and owned race tests

## Outcome
Close the three findings from the review of `599bfde`. A recordless replay requires a complete receipt. The race tests own every process they start, prove that the wrapper exited, and cannot lose their release signal.

## Revisions
Repository: `/Users/oisin/dev/dogsled`. Continue branch `fix/lifecycle-job-claims-r2` **without a base argument**. The retained head is `599bfde037f4716f7c3007a958f4dfa433167c58`. Preserve all commits; do not reset, rebase, or recreate. Earlier briefs: `plans/briefs/lifecycle-job-claims-repair-2.md` and `plans/briefs/lifecycle-job-claims-r2-review.md`.

## Findings (lead verified each at `599bfde`)
1. **Recordless replay accepts an incomplete receipt.** In `correlate` (`src/work.rs:357–366`), the no-record branch returns `Ok(())` before the pane check at line 374. A receipt with a matching `request` but a missing, null, or empty `pane` replays after prune. Fix: validate that the receipt has a non-empty string `pane`, and the correct `id` and `host`, before either replay path. When the Job record exists, it still must match that pane. Add red regressions: after prune, a receipt with a missing, null, or empty pane refuses, and nothing dispatches.
2. **Race-test ownership and exit proof are incomplete.** `Owned::adopt` (`tests/cli.rs:90–131`) holds only a PID. Its drop sends SIGKILL before it checks whether the process is still there. So it cannot reap the wrapper or prove a normal exit. Its comment says the group holds every command the wrapper started. That is false: `jobs::exec` starts the command in its own process group (`src/jobs.rs:201–208`). A failed assertion can therefore leave the FIFO-blocked command running. Fix:
   - Keep the `Child` handle for every process the test spawns.
   - Also track and kill the command's own process group. Find it from the job's recorded pid, or from the wrapper's children; use whatever the source gives you.
   - In the success path, assert that the wrapper exits within a bound and that the PID is gone **before** any fallback kill runs. The kill in `Drop` is cleanup for the failure path only.
   - The `closed` marker (`tests/cli/launch_retry.rs:1615`) proves only that the close branch ran. Assert wrapper exit separately.
   - Fix the false comment.
3. **The FIFO release can lose its signal.** `launch_retry.rs:1612–1614` opens the FIFO read/write, writes `go`, and drops the writer. If the command has not opened its reader yet, closing the last descriptor discards the data. Fix: keep the writer descriptor open until the test reads the acknowledgement, or use a handshake that cannot lose the signal. Apply the same rule to every FIFO helper use in the owned tests.

## Owned paths
`src/work.rs`, `src/work_tests.rs`, `tests/cli.rs`, `tests/cli/launch_retry.rs`, `tests/cli/stop_sweep.rs`. Do not edit `tests/cli/review.rs`; its older pattern is a separate later unit.

## Acceptance
Red before at `599bfde` for the new pane regressions; green after. Run:
```sh
cargo test --test cli jobs:: && cargo test --test cli lifecycle:: && cargo test --test cli launch_retry:: && cargo test --test cli stop_sweep:: && cargo test --test cli review::closed_wrapper_stdout_preserves_full_log_and_child_exit && cargo test --bin dogsled work_tests::
```
After the run, show that no test process stays alive (for example, `pgrep -f` on the rig paths). Prove that a broken release fails within the bound and leaves no process. The pre-commit hook (`mise run check:rust`) must pass once at the final commit. Do not bypass it. Report red and green exit codes and the exact SHA.

## Limits
Local commits only. No push, PR, deployment, cleanup, or dependency changes. Do not change the prune or the lock order. Start at once: write the pane regression first. Write code into files with edit tools, in small pieces; do not draft code in chat. If you make 20 tool calls without an edit, stop and report what blocks you. Use the seeded unit and native done.
