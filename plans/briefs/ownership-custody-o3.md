# O3 binding: how a custodian settles an inherited unit

Read with `plans/briefs/ownership-custody.md`. Architect `-184` design, lead-verified at
`origin/main` `a6f2630`.

## Limit (from the provider API)

The task provider exposes only `inspect`, `claim`, `settle` and `imported`
(`pi/unit-protocol.ts:10–15`). It has no adoption operation. Provider task state belongs to
the session and branch that claimed the unit. So:

- **Resumed original session:** full inheritance. The custodian settles and completes the
  original task through the existing path.
- **Fresh session:** custody, read, and acceptance of the specific result only. The original
  task stays unresolved and status says so.
- **Arbitrary-session adoption:** a product/API gap. Do not solve it with forged provider
  entries, rewritten bindings, a duplicate local task, or edits to the provider.

## Path 1: resumed original session (bound design)

1. On the recording host, resume the original session and task branch (`pi --session`), not a
   fork. Then claim custody (O1) from the replacement runtime.
2. `provider.inspect(unitId)` must return the complete original `UnitRef`
   (`sameRef`, `pi/unit-protocol.ts:25–27`), not only the same unit ID.
3. Deliver the unchanged settlement through `UnitBridge.drain()` (`pi/unit-bridge.ts:144–163`).
4. Acknowledge only after `provider.settle(event)` succeeds. Rejection or uncertainty keeps the
   delivery pending.
5. The custodian reviews the result and calls native `todo done` with evidence. Settlement
   alone never completes the task (`pi/unit-contract.test.ts:167–178`).

Missing session history or the wrong branch blocks completion. A custody receipt cannot
substitute for it.

## Path 2: fresh claimant

A different session may claim custody, read the result and record acceptance of that exact
result as a Dogsled review fact:

```text
{ claimId, revision, execution, resultId, reviewId, evidence, reviewer }
```

- Check current custody and the exact saved returned result first. Stopped or not-started
  outcomes are not returned results (`src/unit_transport.rs:277–287,310–312`).
- A matching replay is idempotent; a conflicting replay is refused.
- O4 projects the receipt into the claimant session as a `dogsled.custody.v1` custom entry
  (`pi.appendEntry`, read back with `getBranch()`). It is an audit receipt, not a task store.
- Status (O2, `src/commands.rs:366–399`): **Custody claimed · result accepted · original task
  completion unverified — resume original session.** Never infer task completion from review,
  acknowledgement or resource closure.

## O3 changes

| File | Change |
|---|---|
| `src/unit_request.rs` | Separate immutable execution correlation from operation authorization. `load()` (`:140–155`) checks `owner_id == owner()` and is shared by commit, abandonment and acknowledgement: use the O1 current custodian for delivery and acknowledgement only. Do not widen commit or abandonment. `settlements()` (`:875`) must include items whose current custodian is the caller. |
| `src/unit_transport.rs` | Delivery envelope and custody-bound acknowledgement. `ExecutionRef`, `Binding`, returned evidence and not-started events stay unchanged. |
| `pi/unit-protocol.ts` | Validate the envelope and custody reference separately from the provider event. Do not extend `TaskProvider`. |
| `pi/unit-bridge.ts` | Before settling, check the current session and exact provider reference. Pass only the unchanged event to `provider.settle()`. Recheck identity after async work with the existing epoch check (`:108–110`). |

Transport:

```text
Delivery { version: 1, custody: { itemId, claimId, revision, recipient }, settlement: <unchanged event> }
AckSettlement { execution: <original ExecutionRef>, eventId, custody: <same reference> }
```

The server compares the custody reference with live O1 authority, never with caller-supplied
identity, and revalidates under the custody lock before acknowledging.

Acknowledgement is scoped to the custody claim. Today one `settlement_ack`
(`src/unit_request.rs:30`, set at `:911`) suppresses delivery for everyone. Keep historical
acknowledgements, but replay the saved event once to a new custodian. Provider duplicate
success acknowledges without another task mutation. Do not regenerate host evidence or clear
the original result.

## O3 tests

- **Authorization:** claimant delivery succeeds; stale revision, previous custodian, reused
  pane and foreign host are refused; origin and evidence stay byte-identical; commit and
  abandonment permissions do not widen.
- **Replay:** a new custodian receives previously acknowledged evidence; duplicate
  acknowledgements are harmless.
- **Bridge failures:** wrong session or branch, provider rejection, lost acknowledgement and
  runtime replacement all leave delivery pending.
- **Real provider recovery:** reopen the persistent original history, settle unchanged
  evidence, check the task is still in progress, then complete with native `todo done`. Run
  with `PI_TODO_SOURCE`; a skipped test is not acceptance.
- **Fresh claimant:** the review receipt survives restart; the original provider task is
  unchanged; status never reports completion from the receipt.

Run `cargo test --test cli unit_requests::`, `cargo test --bin dogsled unit_request`,
the bridge, runtime and contract TypeScript tests, and `npm run typecheck`.

## Before dispatch

Bind O1's runtime identity and lock interface into this unit. A resumed Pi session ID alone
does not prove the replacement runtime holds custody.
