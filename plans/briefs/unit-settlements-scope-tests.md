# Tests that prove the settlements scope (commit 6fac6e1)

**Outcome.** `tests/cli/unit_seeded.rs` proves the behaviour of commit `6fac6e1`: `unit settlements` reads only the caller's unacknowledged units, `unit observe` reads only its one execution, and the service refresh still reads every owner. Test-only change: do not edit any `src/` file.

**Cause it guards.** Before `6fac6e1`, `refresh(fleet)` (`src/unit_request.rs`) ran one `ssh <host> unit-receipt` per committed unit of every owner. On the Mac that was ~30 ssh per prompt and about 900 ssh/min to netcup. The fix adds `Scope::{All, Owner, Execution}` and skips `settlement_ack.is_some()`.

**Edits: add three tests to `tests/cli/unit_seeded.rs`.** Reuse the helpers in that file. Write no new fixture framework.

1. `settlements_reads_only_the_callers_unacknowledged_remote_units`
   - Build three committed remote units in one rig with `refused_unit` + `merge_records`, the same way `invalid_evidence_is_request_scoped_and_other_owners_can_receive_and_ack` (line ~298) does. Point each admission at the remote host as that test does (`attempt["host"]`, `attempt["launch"]["host"]`).
   - Units: (a) owner `w1:p1`, unacknowledged; (b) owner `other-owner`, unacknowledged; (c) owner `w1:p1`, with `unit.settlement_ack` set in its admission record.
   - Make the `ssh` stub log its arguments and answer with a valid receipt for (a). `selected_remote_host_receives_exact_seed_and_durable_receipt_refresh` (line ~1031) shows the stub and `rig.log("ssh.log")`.
   - Run `unit settlements` with `HERDR_PANE_ID=w1:p1`.
   - Assert: `ssh.log` names (a)'s request id; it does not name (b)'s or (c)'s; (a)'s event is in the output.

2. `observe_reads_only_its_own_execution`
   - Two committed remote units for the same owner. Run `unit observe` for the first (`operation(&rig, "observe", &execution_input(&first))`).
   - Assert: `ssh.log` names the first request id and not the second.

3. `service_refresh_still_reads_every_owners_units`
   - Extend the pattern of `existing_service_refresh_imports_receipt_and_does_not_close_unsealed_unit` (line ~1092): a unit whose owner is not the process's `HERDR_PANE_ID`, with a valid host receipt. Run `serve --once --interval 1`.
   - Assert: that unit's `unit.returned` is set after the run.

**Do not edit** `invalid_host_evidence_never_falls_back_to_cached_return_projection`, or any existing test.

**Owned paths.** `tests/cli/unit_seeded.rs` only.

**Acceptance.** `cargo test --locked --test cli unit_seeded` exits 0. Each new test fails if you temporarily pass `Scope::All` in `settlements`/`Observe`. Check that once, revert it, and say in your reply that you did.

**Checks.** `cargo fmt --check`; `cargo clippy --all-targets -- -D warnings`. The pre-commit hook runs `mise run check:rust`; never bypass it.

**Pace.** Read only this brief, `tests/cli/unit_seeded.rs`, `tests/cli.rs` (the `Rig` helpers), and the `refresh` function in `src/unit_request.rs`. Start writing the first test within your first five turns. If blocked for 10 minutes, reply `blocked: <reason>`.

**Publish.** One commit on `fix/unit-settlements-scope`: `test(unit): prove settlements and observe read only their own units`. Before you push, check that `origin/fix/unit-settlements-scope` is still at the SHA you started from. If it moved, reply `blocked`. Push. No PR, merge, release or install. Reply with the head SHA, the three test names, and the check results.
