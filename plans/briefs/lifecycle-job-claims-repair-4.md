# J1 repair 4: tests own and reap every process they start

## Outcome
The J1 race tests and the closed-wrapper test own every process they start. They reap every process and clean up the command's own process group, on success and on failure. This repair changes tests only. The product fixes passed review at `a6970b2` and must not change.

## Revisions
Repository: `/Users/oisin/dev/dogsled`. Continue branch `fix/lifecycle-job-claims-r2` **without a base argument**. Retained head: `a6970b2b1e34ce9174c88109a370cb51c588f268`. Preserve all commits. Do not reset, rebase, or recreate the branch. Earlier briefs: `plans/briefs/lifecycle-job-claims-repair-3.md` and `plans/briefs/lifecycle-job-claims-r3-review.md`.

## Root cause (lead verified at `a6970b2`)
The fake Herdr in `dispatch_order_herdr` (`tests/cli/launch_retry.rs:1543–1590`) starts the real wrapper with a shell `&`. This means the test is not the wrapper's parent:

- `Owned::adopt` (`tests/cli.rs:116–122`) holds only a PID. It cannot reap the wrapper.
- `process_gone` (`tests/cli.rs:300–319`) counts a zombie as gone. Nothing proves that a process was reaped.
- `jobs::exec` starts the command in its own process group (`src/jobs.rs:201–208`). The wrapper-group kill does not reach that group.
- The comment at `tests/cli.rs:128` says the group covers every command the wrapper started. That is false.
- The `NoClose` test (`launch_retry.rs:1788–1815`) never guards the command group.
- `tests/cli/review.rs:34–36` has the same gap and the same false comment.

## Required
1. **The test is the parent.** Each `dogsled exec` wrapper that a test needs to own is spawned by the test as a `std::process::Child`. Do not start it from inside the fake Herdr. Suggested approach (you may choose another one that meets the requirement):
   - The fake Herdr writes the wrapper argv to a file.
   - For `After`, `During`, and `NoClose`, the fake Herdr then blocks on a FIFO.
   - The test runs `host launch` through the bounded `dispatch` helper.
   - The test reads the argv and spawns the wrapper itself in its own process group.
   - The test releases the fake Herdr only at the point each order requires.
   - The `Before` order keeps its meaning: the dispatch fails before any wrapper exists.
2. **Reap proof.** On success, the test proves the wrapper exited by its own action. It waits on the `Child` (`try_wait` with a deadline) and gets an exit status, before any fallback kill. `process_gone` must not count a zombie of a child the test owns as gone. Remove that acceptance, or limit it to processes the test cannot reap, and document which ones.
3. **The command group.** Each test command that can block writes its own PID first, for example `echo $$ > "$RIG/command-pid"`. That PID leads the command's process group. The guard kills that group and waits for it on every path, including failure and timeout. This covers the `NoClose` test and `review.rs`.
4. **Comments.** Every comment must state what the code actually covers. Fix `tests/cli.rs:128` and `review.rs:34–35`.
5. **`tests/cli/review.rs` is now in scope.** Keep the closed-wrapper test's own-and-reap change and complete it under the same rules.

## Owned paths
`tests/cli.rs`, `tests/cli/launch_retry.rs`, `tests/cli/review.rs`, `tests/cli/stop_sweep.rs`. Do not edit `src/`. If a test needs a product change, stop and report.

## Acceptance
```sh
cargo test --test cli jobs:: && cargo test --test cli lifecycle:: && cargo test --test cli launch_retry:: && cargo test --test cli stop_sweep:: && cargo test --test cli review:: && cargo test --bin dogsled work_tests::
```
Also prove the following:
- **Failure path.** Break one release on purpose, for example skip the `go` write. The test then fails within its bound, and afterwards no wrapper or command process remains (`pgrep -f` on the rig path). Restore the release afterwards.
- **No strays.** After the full run, no test process remains, and there are no zombies of the test binary (`ps -o stat=`).
- **Hook.** The pre-commit hook (`mise run check:rust`) passes once at the final commit. Do not bypass it.

Report each exit code and the exact SHA.

## Limits
Local commits only. No push, PR, deployment, cleanup, or dependency changes. Start at once. Make the first edit in `tests/cli.rs` or `launch_retry.rs`. Write code into files with edit tools, in small pieces; do not draft code in chat. If you make 20 tool calls without an edit, stop and report what blocks you. Use the seeded unit and native done.
