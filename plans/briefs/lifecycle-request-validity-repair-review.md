# Review: J2 repair delta (e1c1062..33ee4d1)

Read-only review. Branch `feat/lifecycle-request-validity`, repository `/Users/oisin/dev/dogsled`.

Review only `git diff e1c1062 33ee4d12a0c91ea389bd3af6ef76a593e044c4e4`. Block only on a real defect in this delta. Put older patterns, style and unrelated concerns under "Follow-ups"; they do not block.

## Ignore

`Cargo.toml` (+`[workspace]`) is a known stray edit. A separate writer reverts it. Do not review or block on it.

## Check that each earlier P1 finding is closed

1. `src/jobs.rs` `exec`: validity is rechecked immediately before `capture`. On refusal no process starts, the claim stays, and no terminal "ran" outcome is written.
2. `src/work.rs` `launch_job`: validity is rechecked after pane allocation, before `publish_claim` and dispatch. On refusal nothing dispatches, no receipt is written, `prepare::used` is not called, and only the pane this attempt allocated is retired. Check that the new `retire_pane` helper cannot close a pane this attempt did not allocate (caller, focused, agent-owned, busy, changed terminal).
3. `src/node.rs` `reserve`: validity is checked before `live_reservations` and before the early return for an existing hold. A refused request leaves `placements.json` byte-identical and never returns `reserved: true`, for both new and existing holds.
4. `src/jobs.rs` `json_files` (bootstrap only): an unreadable directory or an unreadable or unparsable record refuses with `job validity clock is unavailable; new execution refused`. A missing directory is still empty. No other caller changed behaviour.

Also check that each of the four regressions actually exercises its fix (it would fail without it), and that nothing else in the delta breaks a check point or the refusal rules: a refusal may record its error and advance the clock, but must not allocate, dispatch, release ownership or write terminal evidence.

## Evidence

The writer reports: red run with fixes stashed 4 failed (`request_validity`) and 2 failed (`work_tests`); fmt, clippy and full `cargo test` (656 passed) exit 0; pre-commit hook passed. The lead has not verified that these ran at exactly `33ee4d1`. Treat them as reported, not proven. Do not rerun the full suite. You may run single targeted tests if a finding depends on it.

## Output

`PASS` or `BLOCK`, then findings with file:line, then follow-ups. No edits, commits or publication.
