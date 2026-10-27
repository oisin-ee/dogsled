# Review J1 repair 5 (delta only)

## Outcome
A PASS or BLOCK verdict on integrating J1 into local `main` at `7ca7b446bac1a9ea910c76a18c1c62f2f95e71e5`. Read-only. The user chose a review of this delta only.

## Revisions
- Repository: `/Users/oisin/dev/dogsled`, branch `fix/lifecycle-job-claims-r2`.
- **Review `c76711c..7ca7b44` only.** It is one commit and changes only `tests/cli.rs` (+63/−3). `git merge-tree` against local `main` is clean.
- The rest of `4dbbe3d..c76711c` was reviewed in `plans/briefs/lifecycle-job-claims-r4-review.md`. That review left one finding open and confirmed every other point. Do not re-review the rest.
- Contract: `plans/briefs/lifecycle-job-claims-repair-5.md`.

## The finding this change must close
At `c76711c`, `Background::drop` (`tests/cli.rs:206–211`) joined the output reader before the `owned` field dropped. A command that never ended therefore blocked the drop past its bound.

## Check
1. `Background::drop` now calls `self.owned.terminate()` before it joins the reader. Confirm that `terminate` kills the group and reaps a spawned child before it returns, so the command's pipes close.
2. **Idempotence.** The `terminated` flag makes the later `Owned` field drop a no-op. Confirm that no path signals a PID after a reap, which could hit a reused PID. Confirm that the flag is set on every constructor.
3. **Regression test** `dropping_a_background_command_kills_it_before_the_guard_joins_its_reader`:
   - Would it fail at `c76711c`? The writer says it does, which is red before the fix.
   - Is it bounded on both paths?
   - On the failure path, does its fallback `killpg` leave no process behind?
   - Is `status --watch --interval 3600` a valid command that never ends on its own?
4. **True comments.** No comment claims coverage the code lacks.

## The writer's claims (untrusted; do not rerun)
- acceptance chain exit 0 (14/6/48/12/5/9);
- `cargo test --test cli` 297 passed;
- fmt and clippy clean;
- the hook passed;
- no stray processes.

## Reply
PASS or BLOCK. For each finding, give its severity, its file:line at `7ca7b44`, and the smallest fix. Say clearly whether a finding blocks integration or can become a later unit.

## Limits
Read-only. No edits, test runs, builds, publication, or cleanup. Use the seeded unit and native done.
