# Review J1 repair 4 before integration

## Outcome
A PASS or BLOCK verdict on integrating J1 (job claims) into local `main`, judged at an exact revision. Read-only.

## Revisions
- Repository: `/Users/oisin/dev/dogsled`.
- Review **`4dbbe3de5950d29837f4bfd146c7eb56fc23a0b1..c76711cd8e0b0750e92008fbb967fcc1f9d0b540`** on branch `fix/lifecycle-job-claims-r2`.
- Focus on the two new commits, `a6970b2..c76711c` (`af038b3`, `c76711c`). They change tests only: 4 files, +319/−167, and nothing under `src/`.
- `git merge-tree` against local `main` is clean.
- Contract: `plans/briefs/lifecycle-job-claims-repair-4.md`.
- Previous review: `plans/briefs/lifecycle-job-claims-r3-review.md`, which recorded two P2 findings at `a6970b2`.
- The product code passed review at `a6970b2`. It has not changed, so do not re-review it beyond confirming that.

## Findings this change must close
1. **The test is the parent.** Each `dogsled exec` wrapper the race tests depend on is spawned by the test as a `Child`, not backgrounded by the fake Herdr. Check `dispatch_order_herdr` and its callers in `tests/cli/launch_retry.rs`.
2. **Reap proof.** On success, the test gets the wrapper's own exit status through `try_wait` with a deadline, before any fallback kill. Then:
   - Check where `process_gone` still treats a zombie as gone (`tests/cli.rs:377`). The writer says this applies only to adopted processes, which the host's init reaps. Confirm that no owned child relies on it.
3. **Command group.** Each blocking command writes `$$` to `command-pid`, and the test owns that group. Check every path, including the failure and timeout paths, `NoClose`, and `tests/cli/review.rs`. The command processes are `Owned::adopt`ed (`launch_retry.rs:1709,1783,1842`). Confirm that the guard kills their group and waits for it.
4. **True comments.** The false comment that was at `tests/cli.rs:128`, and the one at `review.rs:34–35`, are corrected. No comment claims coverage the code lacks.

## Also check
- **The remaining background subshell** at `launch_retry.rs:1605`: `( printf 'closed\n' > "$RIG/wrapper-closed" ) &`. Can it outlive its test or hang it?
- **Spawn sites outside the brief.** Commit `c76711c` converts two other raw spawn sites in `tests/cli.rs` (the peer rendezvous and watch tests) to `Owned`. The file is in scope, but the brief did not name these sites. Judge whether they are correct.
- **No weakened assertions.** Compare the test assertions before and after the change.
- **The writer's report is untrusted.** It claims:
  - acceptance exit 0: jobs 14, lifecycle 6, launch_retry 48, stop_sweep 12, review 5, work_tests 9;
  - full suite 319/296/6;
  - the deliberately broken `go` write failed within its 20 s bound and left no process;
  - no stray processes or zombies afterwards;
  - five runs, all passing;
  - the hook passed at both commits.

  Do not rerun anything. Read the tests and judge whether they prove these claims.

## Reply
PASS or BLOCK. For each finding, give its severity, its file:line at `c76711c`, and the smallest fix. Say clearly whether a finding blocks integration or can become a later unit.

## Limits
Read-only. No edits, test runs, builds, publication, or cleanup. Use the seeded unit and native done.
