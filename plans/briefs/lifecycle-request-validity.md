# J2: enforce recorded job-request validity

## Outcome
A job request older than 30 days, created in the future beyond the skew limit, or without validity evidence can never start new execution. This holds after its Job record and receipt are deleted, and after a host clock moves back. Running jobs are never killed for age.

## Revisions
Repository: `/Users/oisin/dev/dogsled`. Base: **the local `main` revision that integrates J1 repair 2** (`fix/lifecycle-job-claims-r2`). The lead fills in the exact SHA at dispatch. Do not start against an earlier base.
Design source: the AJ2 architect report, summarized here. Approved plan: `plans/lifecycle-cleanup.md` section 7, including "J2 decisions".

## Design (approved; do not reopen)
1. **Origin.** Add an optional versioned `validity { version: 1, created_ms }` to `Action::Job`. Stamp it once in `runtime::request`, which serves both CLI and MCP (`src/runtime.rs:151–155,326–340`; `src/mcp.rs:257–269`). Retries keep it byte-identical. Copy it into the Job claim and the self-sufficient receipt. Include it in the immutable-field comparisons in `publish_claim`, `pending_claim`, and receipt correlation. The wrapper reads it from its claim and never stamps one. Ticket times, reservation times, and ID text are not evidence.
2. **Check points.** Use fresh time at each check, under the lock already held there:
   - queue insertion (`queue`);
   - admission (`admissions/<id>`), before placement, preparation, or dispatch;
   - host reservation (`host`), before capacity changes (`src/node.rs:150–188`);
   - host launch (`launches/<id>`), before new ownership, pane allocation, or dispatch; recheck after blocking preparation or allocation;
   - wrapper (`jobs/<id>-exec → jobs/<id>`), before startup is published, and again just before capture if pane reporting can cross expiry.
   A refusal may record its error and advance the clock. It must not allocate, dispatch, release ownership, or write terminal evidence.
3. **Receipt replay.** A correlated receipt replay stays allowed after expiry. It is read-only: it returns the saved result and never dispatches. Keep the J1 receipt and claim regressions.
4. **Rollback-safe clock.** Use a host-owned `job-validity-clock.json` `{ version: 1, high_water_ms }` under `job-validity-clock.lock`, through `Ledger::with_lock` and `Ledger::write`. For each decision: `effective_now = max(high_water, wall_clock)`, persist it before returning (also when refusing), then expire when `effective_now - created_ms > 2_592_000_000`. Exactly 30 days is still valid. Never advance the clock from a received timestamp. The clock lock is a short leaf lock: it calls no admission, launch, queue, or job lock function.
5. **Bootstrap.** If the clock file is missing, create it at the later of the current wall clock and the newest timestamp in this host's own records (Job records, receipts, admissions, tickets). A corrupt, unsupported, or unwritable clock refuses execution.
6. **Future skew.** Refuse when `created_ms > wall_clock + 300_000`. At origin, refuse new issuance while the wall clock is behind the persisted high-water value by more than the same bound. Never clamp or rewrite `created_ms`.
7. **Peer protocol.** Add a versioned job-validity flag to reservation, launch, and the generated `exec` command, with a matching acknowledgement. An old peer rejects the unknown flag; the caller reports an unsupported validity protocol and does not fall back. A new peer refuses a job launch without validity before it creates a claim or pane. Agent and place traffic keep their current protocol. Reuse the fixed refusal codes in `src/error.rs:77–113`.
8. **Legacy.** Use these refusals:
   - `job request validity is unprovable; new execution refused`
   - `job request is older than 30 days; new execution refused`
   - `job request creation time is ahead of this host clock; new execution refused`
   - `job validity clock is unavailable; new execution refused`
   Legacy queue tickets may fail explicitly. Their admissions, preparations, and Job ownership remain. Completion, output, stop, and inspection stay available.
9. **Scope.** Jobs only. Agent and prepared-unit requests are unchanged; R1 extends validity to them later.

## Owned paths
`src/jobs.rs`, `src/work.rs`, `src/runtime.rs`, `src/ledger.rs`, `src/cli.rs`, `src/main.rs`, `src/hosts.rs`, `src/node.rs`, `src/commands.rs`, `src/error.rs`, `src/mcp.rs`, `src/work_tests.rs`, `src/board.rs` (Job fixture only), `tests/cli/request_validity.rs` (new), `tests/cli/jobs.rs`, `tests/cli/launch_retry.rs`, `tests/cli/remote_diagnostics.rs`, `tests/cli.rs`.

## Red regressions first
Write these before the fix and record that they fail at the base:
1. CLI and MCP issuance keep one timestamp through queueing, retries, admission, claim, and receipt. A changed timestamp conflicts.
2. Exact 30-day boundary; 30 days + 1 ms; 5 minutes ahead (allowed) and 5 minutes + 1 ms ahead (refused); missing validity; corrupt clock; failed clock write. Each refusal has no execution effect.
3. Expiry survives restart and clock rollback, including after the Job record and receipt are deleted.
4. Bootstrap: a missing clock starts at the newest local record timestamp when that is ahead of the wall clock.
5. Peer boundary: new caller and old peer, old caller and new peer. Both give the clear refusal, with no fallback and no early reservation.
6. A running job survives age expiry. An expired request with a matching receipt replays the saved result without dispatch.
Keep the J1 receipt, claim, and race tests, and `interrupted_job_admission_preserves_running_record_without_relaunch`. Use `DOGSLED_TEST_NOW_MS` and the clock-file support in `Rig::cmd` (`tests/cli.rs:183–214`). Use the bounded process harness from J1 repair 2; do not use detached wrappers.

## Acceptance
```sh
cargo test --test cli request_validity::
cargo test --test cli jobs::
cargo test --test cli launch_retry::
cargo test --test cli remote_diagnostics::
cargo test --test cli lifecycle::
cargo test --test cli mcp::
cargo test --bin dogsled work_tests::
```
The pre-commit hook (`mise run check:rust`) must pass once at the final commit. Do not bypass it. Report red and green exit codes and the exact SHAs.

## Limits
Excluded: C8 and R1 deletion, cleanup or prune changes, retry policy, agent validity, ownership release, dependencies, deployment, live migration, push, and PRs. Local commits only. Use the seeded unit and native done. If you make 20 tool calls without an edit, stop and report what blocks you.
