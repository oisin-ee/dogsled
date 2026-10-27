# J1 repair 6: serialize the regression fallback through the guard

## Outcome
The bounded-drop regression's timeout fallback never sends a signal after the command has been reaped. Fallback signalling and reaping go through one owner. Tests only.

## Revisions
- Repository: `/Users/oisin/dev/dogsled`.
- Continue branch `fix/lifecycle-job-claims-r2` **without a base argument**. The retained head is `7ca7b446bac1a9ea910c76a18c1c62f2f95e71e5`.
- Preserve all commits.

## Finding (lead verified at `7ca7b44`)
In `dropping_a_background_command_kills_it_before_the_guard_joins_its_reader`, at `tests/cli.rs:1268–1277`, the test caches `background.owned.group` before the drop. On timeout it calls `killpg` with that cached ID, outside the `Owned` guard.

The dropper thread may already have reaped the child and still be joining the reader. In that case the group ID can belong to another process, and the call bypasses `terminated`.

## Required
1. The fallback kill and `Owned::terminate` share one piece of state, so a kill is sent only while the command is known to be unreaped, and never after a reap.
   - One way to do this is a shared, lock-protected kill handle on `Owned`.
   - Any equivalent design is acceptable if it keeps that rule.
2. The regression test must still:
   - stay bounded on both its pass and its fail paths;
   - leave no stray processes;
   - fail at `c76711c` and pass now.
3. Comments must state what the code really does.

## Owned paths
`tests/cli.rs` only. Do not edit `src/` or any other file.

## Acceptance
```sh
cargo test --test cli jobs:: && cargo test --test cli lifecycle:: && cargo test --test cli launch_retry:: && cargo test --test cli stop_sweep:: && cargo test --test cli review:: && cargo test --bin dogsled work_tests::
```
- Run this once and record its exit code.
- Afterwards, no stray test processes.
- The pre-commit hook passes at the final commit. Do not bypass it.
- Report the exact SHA.

## Limits
- Local commits only.
- Start at once with the edit.
- Write code into the file with edit tools, one small piece at a time. Keep your replies to a few lines; do not draft code or long reasoning in chat.
- If you make 20 tool calls without an edit, stop and report.
- Use the seeded unit and native done.
