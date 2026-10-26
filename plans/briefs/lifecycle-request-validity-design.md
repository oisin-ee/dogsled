# AJ2: design recorded job-request validity (J2)

## Outcome
A source-backed design and a bounded writer brief for J2. The design must let an old or unprovable job request never start new execution, including after C8 deletes Job metadata. Read-only; no code.

## Revisions
Repository: `/Users/oisin/dev/dogsled`. Design against `7feffe978b515ae4a9f62e85fed2ae0617bcfd2e` (J1 plus its first repair; local `main` `cbaac83` already holds C7).
A parallel writer is changing job launch receipts on `fix/lifecycle-job-claims-r2` (brief `plans/briefs/lifecycle-job-claims-repair-2.md`). On a successful dispatch, a receipt will carry id, host, pane, command, cwd, owner, and timeouts, so matching replay works after prune deletes `jobs/ID.json`. Assume that change lands, and design J2 to compose with it. Do not depend on its exact code.
Read the approved plan `plans/lifecycle-cleanup.md`, section 7 ("Old requests", the J1/J2 paragraph, clock rollback) and section 8 (J2, C8, R1). Read the earlier retention architect question list in `plans/briefs/lifecycle-retention-contract.md` (historical).

## Approved decisions (do not reopen)
- Record creation time once at origin and propagate it unchanged. Never derive it from request IDs or refresh it on retry.
- Refuse recordless or unprovably valid execution. A legacy record stays readable, but missing validity evidence never permits execution.
- A request older than 30 days cannot start new execution, even when its unresolved ownership records must remain. Age never ends ownership.
- Clock rollback must not reopen an expired request. Validate timestamp ordering.
- A failed transport response alone is not terminal proof. No owner read acknowledgement protocol.
- Job retention (C8) and other-store retention (R1) are separate units. Do not design them here, except where J2 must provide what they need.

## Questions to resolve with file:line evidence
1. **Origin.** Where a job request is created (CLI `run`, MCP, queue ticket, admission) and the single place to stamp creation time. How it reaches the host launch request (`work::Launch`) and the wrapper (`dogsled exec`) unchanged.
2. **Check points.** The exact points that must refuse an expired or recordless request before any side effect: queue admission, host `launch`, receipt replay, and wrapper start. Which lock each check runs under, given the lock order in plan section 6. The J1 execution lock must not be taken inside a launch or store lock.
3. **Rollback-safe expiry.** How a host decides "older than 30 days" so a clock moved back cannot reopen a request. Options include a persisted high-water time or a monotonic floor per host. Pick the smallest sound mechanism and say where it is stored.
4. **Future timestamps and skew.** What to do with a creation time ahead of the host clock: refuse, or allow within a bound. State the bound and the reason.
5. **Peer capability.** Remote hosts run older binaries (v0.23.2). How a new caller and an old peer behave, and how an old caller and a new peer behave. The plan requires a clear "unsupported" result and no downgrade below fence-aware versions. Decide whether a capability flag or a request field version is needed.
6. **Legacy records.** Existing queue tickets, admissions, receipts, and Job records without a creation time: exact refusal behavior and message. Confirm nothing in flight on the fleet today is silently killed. Ownership records stay; only new execution refuses.
7. **Composition.** How J2 composes with the J1 claim checks and the self-sufficient receipts, and what C8 can then delete safely (Job JSON at 30 days, logs at 24 hours). Name any receipt or record field C8 must keep.
8. **Interaction with prepared-unit requests.** Agent/unit requests are not jobs. Confirm J2 is jobs-only, and list what R1 must later extend to preparation, placement, and agent dispatch.

## Deliverable
A reply containing:
- A decision for each question, with file:line evidence at `7feffe9`.
- A writer brief for J2: owned paths (todo draft: `src/runtime.rs`, `src/work.rs`, `src/jobs.rs`, `src/cli.rs`, `src/main.rs`, `src/hosts.rs`, `src/ledger.rs`, `src/work_tests.rs`, `tests/cli/jobs.rs`, `tests/cli/launch_retry.rs`, `tests/cli/remote_diagnostics.rs`, `tests/cli.rs`; correct the list), red regressions to write first, acceptance commands, and what stays out of scope.
- Any point where the approved decisions conflict with the source. Report it as a user decision; do not resolve it yourself.

## Limits
Read-only. No edits, builds, tests, installs, SSH, publication or cleanup. Use the seeded unit and native done.
