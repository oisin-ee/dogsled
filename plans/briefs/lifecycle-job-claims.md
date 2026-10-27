# J1: refuse execution without a durable job claim

## Outcome
A job wrapper runs only its existing matching pending claim. Missing replay evidence never causes an existing job to run twice.

## Cause
`src/runtime.rs:608–634` constructs a job when its record is missing. `src/jobs.rs:300–307` ignores read errors and guards only `started_ms`.
`src/work.rs:318–365` creates a pane before recording job ownership, overwrites existing job state when the receipt is missing, and treats transport failure as terminal.

## Approved behavior
The user explicitly approved refusing recordless or unprovably valid execution. Normal `dogsled run` remains supported; old records remain readable.
This unit fixes durable claims only. J2 adds immutable request age; C8 later deletes metadata. Do not implement retention or claim that J1 alone permits it.

## Edits
- `src/runtime.rs`: remove recordless Job construction. Require the saved claim and validate the requested command/options. Do not replace read errors with a new claim.
- `src/jobs.rs`: own claim validation and short record updates. Under the lifetime execution lock, reload and validate the exact pending claim before starting. Reject missing, corrupt, conflicting, already-started, terminal, and uncertain claims without side effects.
- `src/work.rs`: under the launch-request lock, inspect existing job ownership before pane allocation. Persist first-attempt intent before pane side effects. Do not overwrite or redispatch an existing pending, started, terminal, or uncertain claim when a launch receipt is missing. A matching saved receipt may replay its saved result without execution.
- Keep a pre-allocation intent distinct from an executable pending claim. Publish a matching pending claim only after its pane is known. An interrupted allocation remains uncertain and cannot authorize another allocation.
- On a failed `pane run` response, re-read under the short job-store lock. Never overwrite a wrapper's running or terminal state with the original pending copy. If startup is unproved, retain uncertainty rather than inventing `ended_ms` or a safe retry.
- Keep the existing execution lock lifetime-scoped. Use the short per-job record lock for publication. Never hold a short store lock across pane submission, capture, or arbitrary job lifetime. Never acquire the execution lock inside a held launch/store lock.
- Preserve legitimate wrapper termination, process-group cleanup, timeouts, and final-record-before-pane-close behavior. Update direct-exec tests to seed genuine pending claims rather than weakening these checks.

## Owned paths
`src/jobs.rs`, `src/runtime.rs`, `src/work.rs`, `src/work_tests.rs`, `tests/cli/jobs.rs`, `tests/cli/lifecycle.rs`, `tests/cli/launch_retry.rs`, `tests/cli/review.rs`, `tests/cli.rs` (job fixture helpers only).
Generated paths: none. Do not edit the main checkout, other workers' files, dependencies, or live fleet state.

## Acceptance
Run `cargo test --test cli jobs:: && cargo test --test cli lifecycle:: && cargo test --test cli launch_retry:: && cargo test --test cli review::closed_wrapper_stdout_preserves_full_log_and_child_exit` in your worktree.
Add failing regressions first. Missing/corrupt/mismatched exec claims must create no process/log/pane. Host replay with a missing receipt must not allocate or run again when a claim exists.
Force both wrapper/error-publication orders without sleeps. A running/terminal wrapper record must survive transport error; uncertainty must refuse delayed startup.
Keep `interrupted_job_admission_preserves_running_record_without_relaunch` as a negative control. Keep real timeout and process-group tests.
Report command exit codes and exact tested revision. If piping output, preserve the test command's exit status.

## Checks and authority
Allow the focused acceptance, `cargo fmt --check`, and required pre-commit hooks (`mise run check:rust`). Do not bypass hooks.
No new dependencies, unrelated checks, release, push, merge, deployment, or host cleanup. Existing declarative worktree setup is allowed.
Commit the scoped result in your own linked worktree. Stop and report if the ownership/claim contract cannot be met within scope.

## Base and handoff
Base: `4dbbe3de5950d29837f4bfd146c7eb56fc23a0b1`. Branch: `fix/lifecycle-job-claims`.
Prerequisite: accepted retention architect unit `u-2572ff0d-dad9-4c11-b145-84997b007aee-11` and the four-choice follow-up interview.
Read applicable code-standard architecture/repair rules, the owning source, and the named regressions before editing.
Reply with branch, final SHA, changed paths, red/green evidence, exact commands, and limits. Use the seeded unit and native done protocol.
