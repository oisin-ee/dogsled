<!-- Architect arch-c4c6 output, saved by the lead 2026-10-28. Baseline origin/main 83d63d2; C2 merged since as 61e7f06 (#215). Lead decisions pending: C4a durable grant (default approve), C4b interrupted preparation stays unresolved (default approve), C5 cancel-after-admission = too late (default approve), C6 cleanup authority surface CLI-only (default approve). -->

# Lifecycle C4–C6 writer contracts

**Cause:** placement bypasses admission journaling, cancellation changes only the queue, and preparation release lacks retirement authority and audit semantics.

## Baseline and dispatch conditions

All unqualified source references below name **`origin/main` at `83d63d297debd646a6ed1d6af10bf5e4784ea404`**.

C2 was inspected at **`51a82d5a2702fc0312fc32240901344f9381dfd3`**, [PR #215](https://github.com/oisin-ee/dogsled/pull/215). It remains a prerequisite, not integrated evidence.

| Unit | Required dispatch base | Deliverable |
|---|---|---|
| C4a | Exact integrated C3b revision | Durable placement admission and reservation replay |
| C4b | Exact integrated C4a revision | Saved placement result, acknowledged release, and public resume |
| C5 | Exact integrated C4b revision | Cancellation serialized with admission and evidence-based settlement |
| C6 | Exact integrated C5 revision | Audited retirement of one request’s preparation claim |

The lead must replace these prerequisite descriptions with full committed SHAs before dispatch. Shared files require sequential writers.

This review changed no files or Git state. No build or test ran.

---

## Shared binding requirements

### Required C3 interface

These are **prerequisite requirements**, not claims about implemented C3 code.

| Required capability | C4–C6 dependency |
|---|---|
| **Correlated preparation lookup** | Read the original request, outcome, claim, and disposition without allocating, running setup, or opening a pane. Distinguish absent, incomplete, successful, failed, and disposed records. |
| **Immutable disposition intent** | Persist the exact request identity, selected host, operation, expected claim, and operation identity before transport. Repeated delivery uses that saved envelope. |
| **Host-owned fence and release** | Under the preparation request gate, validate the envelope, persist the dispatch fence, release only the matching claim, and save a correlated acknowledgement. |
| **Caller-owned fence** | Admission, queue recovery, explicit resume, and commitment cannot dispatch a prohibited request. Checking a terminal ticket alone is insufficient. |
| **Independent execution evidence** | A disposition acknowledgement proves fencing/release, not that execution never happened. It must not manufacture `not_started` or modify Job ownership. |

C3 must cover these existing callers:

- Preparation: `prepare::run`, `local`, and `on_host` — `src/prepare.rs:56–183,498–509`.
- Admission and host launch: `work::perform_locked`, `receive_launch`, and `launch` — `src/work.rs:764–875,1196–1283`.
- Release: placement, agent/job launch completion, and native-refusal receipt repair — `src/work.rs:923–970,1163–1185`; `src/launch.rs:1075–1136`; `src/main.rs:688–700`.
- Job wrapper startup: **C2** `src/jobs.rs:1091–1155`.
- Continuation: preserve the established admission → done → launch order at `src/continuation.rs:65–67`; apply C3’s decided distinction between prohibited new dispatch and permitted historical evidence reads.

If C3 cannot provide these contracts, stop before dispatching the dependent writer. Do not recreate disposition policy in each caller.

### Atomic expectations

Each operation captures **one complete expectation**, persists or retains it, and compares later observations against it.

| Operation | Immutable expectation |
|---|---|
| Placement | Entire originating `Request`; selected `Host`, including target; reservation identity; C3 preparation request and returned claim identity |
| Cancellation | Ticket/request pair; complete request; current admission evidence; existing launch/result; cancellation operation identity |
| Retirement | Request ID; exact historical claim and preparation provenance; selected host; actor/authority; custody revision when applicable; reason |

Rules:

1. Later observations may validate an expectation, never replace it.
2. A changed request, host target, custody revision, or claim produces a conflict before release or dispatch.
3. Missing identity remains missing. Never fill historical identity from the current pathname.
4. Use C1’s checkout identity, not path equality, when an operation touches checkout ownership. C1 defines that identity at `src/worktree.rs:26–42`.
5. Keep filesystem capture and validation in the existing owner. C1’s bracketed capture and pinned-directory checks are at `src/worktree.rs:202–230,405–462`.

### Lock order

```text
service admission lock, when already held
  → origin admissions/<request-id>
    → custody, only for the short authority decision
    → origin placement-attempts/<request-id>, when selecting/reserving
    → host preparation-receipts/<request-id>
      → Git common-directory lifecycle lock
        → done
          → launches/<request-id>
            → launch-panes/<pane>
              → short store locks
```

Additional constraints:

- Release custody before host RPCs. Save the authorized intent while custody remains locked; that intent becomes the operation’s authority.
- The capacity branch uses the host preparation gate followed by the existing host-capacity lock. Do not hold the capacity lock during preparation or lifecycle work.
- Never hold `queue` while acquiring admission or making RPCs.
- Keep `prepare::used` a short, matching-claim leaf operation.
- Preserve C2’s startup order and lifetime-lock boundary. C2 takes preparation → repository → nonblocking exec → short Job lock; monitoring retains only exec.

Existing admissions-before-custody examples are `src/unit_request.rs:1013–1075,1100–1136`. C1’s lifecycle lock is `src/worktree.rs:176–187`.

### Writer execution contract

For every unit:

- **Repository:** Dogsled.
- **Generated paths:** none.
- **Publication authority:** commit owned changes only; no push, PR, merge, release, installation, or host-state cleanup.
- **Read first:** this contract, the integrated predecessor diff, the cited owners, and existing CLI fixtures.
- **Checks:** run the named focused regressions before and after implementation. Record command, revision, failure assertion, and final result.
- **Heavy checks:** the lead owns the integrated repository gate. Required commit hooks remain enabled.
- **Failure behavior:** stop on a missing prerequisite, ownership conflict, or consequential design change. Do not widen scope silently.

Use existing real-process fixtures: `Rig`, `Background`, `Owned`, and `Fifo` in `tests/cli.rs:65–203,302–370`. Use FIFO rendezvous or held kernel locks, not sleeps, to establish race order.

---

# C4 — Placement replay

## Owning source and failure windows

| Owner | Current behavior |
|---|---|
| `src/work.rs:1161–1192` | `Action::Place` runs outside the request admission lock. It releases preparation before returning the placement result. |
| `src/work.rs:1130–1145,1285–1289` | `Attempt` lacks placement progress; `save_result` saves only unit results. |
| `src/commands.rs:53–175` | Selection is saved before reservation, but its record contains only the host ID. |
| `src/node.rs:141–199` | Reservation replay recognizes only a live TTL hold. After expiry, the same ID undergoes new admission. |
| `src/prepare.rs:109–211` | Completed preparation outcomes replay; an interrupted receipt without an outcome refuses. |

Concrete interleavings:

| Window | Interleaving |
|---|---|
| Lost preparation reply | Host completes preparation → saves its outcome → reply disappears → origin has no placement `Attempt` containing the result. |
| Lost release acknowledgement | Host releases the claim → acknowledgement disappears → origin has neither a saved placement result nor acknowledged completion. |
| Lost caller result | Origin releases preparation → process exits before returning output → another invocation constructs a new request ID at `src/runtime.rs:61–63`. |
| Reservation acknowledgement gap | Host reserves → reply disappears → TTL expires → retry calls reserve with the same ID → host grants new capacity. |
| Concurrent replay | Two callers reach placement without a shared admission gate → neither owns an authoritative placement phase/result journal. |

### Why split C4

The reservation acknowledgement gap has a different owner from preparation replay: `node` owns capacity; `work` owns caller progress.

Under a literal reading of “no new capacity,” both need repair. **C4a’s durable reservation receipt is a proposed scope clarification requiring approval.** Merely saving more origin fields cannot close a crash between host reservation and origin persistence.

If reservation-reply loss is excluded, narrow the acceptance statement explicitly. Do not claim the broader guarantee using TTL-only holds.

---

## C4a — Capture admission and replay the original capacity grant

**Proposed branch:** `feat/placement-grant-replay`

### Binding design

**Origin admission**

Move `Action::Place` under `admissions/<id>`. Save the full request before reservation transport.

Introduce versioned placement progress on `Attempt`, initially:

```text
recorded
  → selection pending
  → host selected
```

“Host selected” includes a positively correlated reservation grant, not just a host name.

Keep the original selected host and transport target. A changed target must not silently redirect replay.

**Host reservation evidence**

Add a placement-specific, versioned reservation envelope. Keep existing Job reservation behavior unchanged.

The host owns a durable grant record, for example:

```text
placement-grants/<request-id>.json
```

It contains:

- Original request correlation and selected-host correlation.
- Original reservation ID and creation time.
- The immutable grant decision.
- Whether the original capacity hold was materialized.

Under the host capacity lock:

1. Capture eligibility once.
2. Persist the granted intent before publishing the capacity hold.
3. Publish that exact hold, without changing its original timestamp.
4. Persist materialization before acknowledging.

Recovery repairs an interrupted publication using the original reservation identity and timestamp. A materialized grant never refreshes or recreates its expired hold.

An acknowledged historical grant can resume the same admitted placement. It is not a new capacity admission. Recordless or mismatched requests cannot obtain that treatment.

Retain these records in C4. R1 must later own their safe expiry.

**Protocol and fences**

Use an explicit placement-replay protocol envelope on host reservation transport. Reject unsupported peers before reservation or preparation.

The host checks C3’s request fence under the preparation gate before granting or repairing a placement reservation. A retired request must not regain capacity through a delayed reservation RPC.

**Caller coverage**

Cover initial CLI/MCP placement, queued admission, reservation transport retry, and local/remote host reservation.

All production `commands::place_with` calls currently originate at `src/work.rs:1331–1338`. Preserve Agent and Job semantics at that shared boundary.

### Exact owned files

```text
src/work.rs
src/unit_request.rs
src/commands.rs
src/node.rs
src/hosts.rs
src/cli.rs
src/main.rs
src/work_tests.rs
tests/cli/place.rs
tests/cli.rs
```

`src/unit_request.rs` is owned only for initializing the extended `Attempt`. Do not change unit settlement behavior.

### Red-before tests

| Named test | Setup, interleaving, assertions |
|---|---|
| `place::placement_request_is_recorded_before_reservation` | Stop the fake SSH reserve command at a FIFO. Inspect origin storage before releasing it. Assert the complete request and selected-host expectation already exist. |
| `place::lost_reservation_reply_replays_one_capacity_grant` | Delegate to the real host reserve command, discard its first successful reply, then retry through admission. Assert one grant identity, one capacity hold, and unchanged creation time. |
| `place::reservation_reply_replay_after_ttl_does_not_readmit` | Lose the reply after reservation. Advance the debug clock beyond TTL and make the host full. Resume the same grant. Assert no new hold, timestamp refresh, or new admission decision. |
| `place::concurrent_reservation_replays_publish_one_hold` | Start two same-request host deliveries behind a held host lock. Release it. Assert identical correlated acknowledgements and one hold. |
| `place::reservation_replay_rejects_changed_identity_and_old_peer` | Table-test changed request, changed host target, unsupported version, and missing replay evidence. Assert no reservation, preparation, or pane side effect. |

Also inject crashes after granted-intent persistence and after hold publication. Recovery must use the original timestamp in both cases.

### Acceptance

```sh
cargo test --test cli place::
cargo test --bin dogsled node::
cargo test --bin dogsled work_tests::
cargo test --test cli request_validity::
```

**Lead decision:** approve the durable-grant extension and its later R1 retention ownership. Recommended default: approve it to preserve the literal no-new-capacity guarantee.

---

## C4b — Resume preparation, save the result, acknowledge release

**Proposed branch:** `feat/placement-resume`

### Binding design

Extend the same placement progress:

```text
recorded
  → host selected
  → prepared
  → result saved
  → release acknowledged
  → completed
```

Use typed states or equivalent validated data. Impossible combinations must refuse, not be repaired through guessed defaults.

**Preparation and result**

After host selection:

- Never call placement/reservation again.
- Preserve the original checkout/preparation envelope.
- If the host saved a completed preparation outcome, recover it through C3.
- If preparation is incomplete, preserve uncertainty. Never rerun setup or open another pane.
- Save the complete public placement result before requesting release.

A saved result contains the original request ID, host, reservation, path, and pane ID. Preserve current branchless placement behavior: it reserves capacity without preparing a worktree or creating a pane (`src/work.rs:1360–1376,1412–1420`).

**Release**

Persist a C3 disposition intent referencing the saved result and exact claim.

Only a matching acknowledgement advances `release acknowledged`. A lost or mismatched acknowledgement leaves the result durable and release pending.

Do not return an ordinary completed receipt while release is pending. Return an explicit recoverable status containing the request ID and saved result.

Completed replay returns the stored result without capacity checks, setup, pane allocation, or preparation RPCs.

**Public resume**

Add:

```text
place --resume <request-id>
```

It loads the original request. It must not call `runtime::request`, rediscover repository identity, or replace placement fields from current arguments.

Reject `--resume` combined with host, branch, base, or repo overrides. Permit wait/budget controls because they do not change request identity.

Expose equivalent MCP resume through `PlaceArgs`, with conflicting placement fields rejected.

Include the request ID in successful, queued, pending-release, and error responses. For CLI calls, report it on stderr before side effects so interrupted callers can retain it without corrupting JSON stdout.

**Recovery**

Initial submission, explicit resume, and queued recovery use the same locked state machine.

Update `admit` to recover interrupted placement attempts. Do not extend its Job replay policy: the existing Job negative control remains binding.

A resumed request must not wait behind its own ticket. Unreserved work still respects other pending requests.

### Caller inventory

| Entry | Required route |
|---|---|
| CLI `place` | `src/runtime.rs:344–365` → placement state machine |
| MCP `place` | `src/mcp.rs:190–194,493–503` → same state machine |
| New CLI/MCP resume | Load saved request → same state machine |
| Queue/service recovery | `src/work.rs:1582–1662,1688–1737` → same state machine |
| Local/remote preparation | Existing `prepare_launch` and C3 transport; no parallel implementation |

### Exact owned files

```text
src/work.rs
src/work/placement.rs                 # new cohesive placement-progress owner
src/runtime.rs
src/cli.rs
src/mcp.rs
src/work_tests.rs
tests/cli/place.rs
tests/cli/mcp.rs
```

Do not modify C3’s host disposition policy. A missing C3 capability is a prerequisite failure.

### Red-before tests

| Named test | Setup, interleaving, assertions |
|---|---|
| `place::lost_preparation_reply_resumes_saved_host_outcome` | Real host preparation completes; SSH drops the response. Resume the saved request. Assert one setup, one worktree opening, one grant, and matching result. |
| `place::lost_release_ack_resumes_without_preparing_again` | Host persists fence and releases claim; transport drops acknowledgement. Resume. Assert the original result and disposition replay, with unchanged allocation counters. |
| `place::placement_crash_boundaries_replay_one_result` | Table-test crashes after preparation recovery, result save, acknowledgement save, and completion save. Restart each case. Assert monotonic progress and identical final result. |
| `place::concurrent_cli_resume_and_service_replay_once` | Hold the request admission lock, start both contenders, then release. Assert one result, one matching release, and no duplicate ticket or allocation. |
| `place::resume_refuses_unknown_or_changed_requests_without_effects` | Table-test unknown ID, wrong action, conflicting CLI fields, changed claim, and incomplete preparation receipt. Assert refusal or unresolved status without new effects. |

Add a branchless case and an MCP parity case. At least one backend regression must fail on duplicate effects or missing durability, not merely because `--resume` is initially absent.

### Acceptance

```sh
cargo test --test cli place::
cargo test --test cli mcp::
cargo test --bin dogsled work_tests::
cargo test --test cli launch_retry::
```

Preserve:

```sh
cargo test --test cli launch_retry::interrupted_job_admission_preserves_running_record_without_relaunch
```

**Lead decision:** confirm that interrupted host preparation remains unresolved when no completed outcome exists. Recommended default: preserve refusal; successful exactly-once recovery is not provable in that window.

---

# C5 — Cancellation under admission

**Proposed branch:** `fix/admission-cancellation`

## Owning source

| Owner | Current behavior |
|---|---|
| `src/work.rs:383–395` | Cancellation accepts only pending tickets and changes their state under `queue`. |
| `src/work.rs:1196–1283` | Dispatch uses the request admission lock but does not consult cancellation intent. |
| `src/work.rs:1582–1654` | Queue claim, execution, and outcome publication are separate operations. |
| `src/unit_request.rs:427–503` | `not_started` derives from terminal ticket state and absence of saved launch/result. |
| `src/runtime.rs:637`; `src/mcp.rs:484–489`; `src/sidebar/mod.rs:141–143` | Three direct cancellation callers; Pi’s work panel invokes the CLI at `pi/work-panel.ts:181`. |

**Important qualification:** main already protects the narrow pending→admitting race with the queue-state check at `src/work.rs:1618–1624`. Do not describe that check as absent. The missing contract spans admission, C4 resume, durable cancellation, and host release.

## Concrete interleavings

| Window | Interleaving |
|---|---|
| Cancel versus C4 resume | Resume captures a pending request → cancel marks the queue cancelled → resume proceeds without a request-level cancellation fence. |
| Cancel during preparation | Admission owns the request and prepares remotely → cancel arrives → whichever acquires admission first must determine future dispatch. Queue state alone cannot interrupt or settle this safely. |
| Lost launch reply | Origin saves `Launch` → host may start → response disappears → ticket becomes retryable/pending → cancellation must retain “possibly executed.” |
| Outcome publication | Dispatch returns → cancellation records intent → a stale queue finalizer overwrites cancellation unless finalization shares admission ordering. |
| Host unavailable | Cancellation is durable → disposition RPC fails → service restarts. Release must remain pending; neither a retry nor `not_started` may be invented from the failure. |

## Binding implementation design

### Durable cancellation

Keep cancellation policy in `work`.

`cancel` captures the ticket/request identity, acquires `admissions/<request-id>`, and revalidates the ticket under a short queue lock.

Persist a versioned cancellation intent and C3 caller fence before reporting success. Use one stable operation identity for retries.

Keep these facts separate:

```text
future admission: prohibited
execution outcome: not_started | possibly_executed | already known
preparation release: not_needed | pending | acknowledged
```

`cancelled` must mean no further admission. It must not mean the command never ran.

Cancellation does not stop a running process, close a pane, delete a checkout, or terminalize a Job.

### Serialize all admission transitions

For each ticket, hold its request admission lock across:

```text
validate current request/fence
→ short queue claim
→ perform_locked
→ short queue outcome publication
```

Queue locks end before preparation or RPCs.

Explicit C4 resume and synchronous commitment use the same fence check. Service recovery must not reset a cancelled request to pending or enqueue it again.

Keep `cancel` ledger-owned: it records intent and returns structured status. The existing service lane performs pending host dispositions through C3. This avoids requiring network work inside the sidebar cancellation call.

### Evidence for `not_started`

Retain the existing safe distinction: a saved launch may have reached the host.

Accept only:

- An admission-locked proof that no launch/result was saved, under the supported write-before-dispatch invariant, combined with a durable prohibition on future dispatch.
- Or existing, positively correlated host no-start/refusal evidence.

A failed/cancelled ticket alone is not proof. Neither is a release acknowledgement, timeout, missing pane, or absent Job record.

`settle_unlaunched` must reload the ticket under admission, rather than authorize settlement using its earlier queue snapshot.

Save a stable no-launch witness before creating the existing `NotStarted` event. Keep settlement delivery and acknowledgement semantics unchanged.

If origin evidence conclusively proves no dispatch, host release can remain independently pending. An unreachable preparation host does not invalidate that proof—but it also does not prove release.

### Caller and recovery coverage

Update:

- CLI, MCP, Rust sidebar cancellation, and their displayed result wording.
- Service interrupted-ticket recovery and outcome publication.
- `unit_request::recover`, commitment, observation, and `settle_unlaunched`.
- C4 explicit placement resume and cancellation of pending-release placement.
- Existing host-refusal settlement without weakening its provenance checks at `src/unit_request.rs:721–782`.

Pi’s existing CLI invocation remains valid; no Pi schema change is required.

## Exact owned files

```text
src/work.rs
src/unit_request.rs
src/runtime.rs
src/mcp.rs
src/sidebar/mod.rs
src/work_tests.rs
tests/cli/unit_requests.rs
tests/cli/mcp.rs
tests/cli/place.rs
```

Do not change Job retry policy, custody policy, or host disposition internals.

## Red-before tests

| Named test | Setup, interleaving, assertions |
|---|---|
| `unit_requests::cancel_before_admission_fences_every_replay` | Queue a committed unit. Cancel while admission is held externally, then release it. Invoke service recovery, commitment replay, and the relevant resume path. Assert no dispatch and one stable `not_started` event from saved proof. |
| `unit_requests::dispatch_before_cancel_preserves_possible_execution` | Pause host dispatch after the origin saved `Launch`; start cancellation; let the host start and lose its reply. Assert cancellation never produces `not_started`, and launch/Job evidence remains intact. |
| `unit_requests::cancel_during_admitting_orders_before_finalization` | Pause admission with a FIFO and race cancellation. Test both lock winners. Assert outcome publication cannot erase the final cancellation decision or discard a known result. |
| `unit_requests::unreachable_host_keeps_cancel_release_pending` | Prepare remotely, cancel, fail disposition transport, restart the service, then restore transport. Assert persistent cancellation, no launch retry, and eventual release of only the matching claim. |
| `unit_requests::terminal_ticket_without_no_launch_proof_stays_unresolved` | Table-test missing/corrupt admission, saved launch, mismatched request, and release acknowledgement without execution proof. Assert no `NotStarted` event or provider settlement. |

Add deterministic crashes immediately after cancellation-intent persistence and after host acknowledgement persistence. Restart must repair the queue projection without re-admission.

Preserve positive coverage for cancellation before host selection, duplicate cancellation, and existing host-refusal settlement.

## Acceptance

```sh
cargo test --test cli unit_requests::
cargo test --test cli place::
cargo test --test cli mcp::
cargo test --bin dogsled work_tests::
cargo test --bin dogsled sidebar::
```

Regression controls:

```sh
cargo test --test cli jobs::
cargo test --test cli launch_retry::interrupted_job_admission_preserves_running_record_without_relaunch
```

## Lead decisions

**Cancellation after admission:** recommended default is “too late” for an already completed admission, preserving its result. Cancellation of pending/admitting work records the fence and reports possibly executed when appropriate. It never becomes a stop operation.

**Authority:** preserve existing cancellation authorization in C5. Do not introduce a new custody-transfer or operator-authorization policy while repairing ordering.

---

# C6 — Request-scoped retirement

**Proposed branch:** `feat/preparation-retirement`

## Owning source

| Owner | Current behavior |
|---|---|
| `src/prepare.rs:24–30,480–495` | Claims contain a tree and optional request ID; records distinguish requests sharing a path. |
| `src/prepare.rs:185–211` | Preparation outcome is separate historical evidence; incomplete outcome remains uncertain. |
| `src/prepare.rs:549–560` | `used` already releases only matching path/request records. Preserve this independence. |
| `src/unit_request.rs:1013–1075` | Admission precedes custody during an authority-sensitive mutation. |
| `src/custody.rs:26–46,308–314,336–364` | Custody records carry revision/session identity and provide a locked check. |

C2 adds independent Job holds: **C2** `src/jobs.rs:414–440`; cleanup reloads them under its final ownership check. Retiring preparation must not remove those holds.

## Concrete interleavings

| Window | Interleaving |
|---|---|
| Shared pathname | Requests A and B hold the same checkout → retire A → B must remain byte-for-byte unchanged and continue holding the checkout. |
| Late dispatch | Retirement captures A → a delayed launch or wrapper attempts A → the host preparation gate orders dispatch against the retirement fence. |
| Replacement claim | Retirement captures A’s claim → waits → stored claim changes → retirement must refuse, not adopt the replacement. |
| Lost acknowledgement | Host saves retirement fence and releases A → response disappears → origin retries the same intent and receives the same receipt. |
| Custody movement | Caller captures custody revision N → another claimant obtains N+1 → retirement authority must fail comparison, not use newly observed authority. |

## Binding implementation design

### Command and authority

Add:

```text
retire-preparation <request-id> [--host <host>] --reason <text>
```

Recommended authority modes:

- **Owner/current custodian:** validate recorded origin or current custody using live caller identity. Capture the authority tuple atomically under admission → custody.
- **Explicit cleanup workflow:** a separately authorized CLI mode records the operator and reason. Owner absence alone never grants this authority.

Do not accept a caller-supplied actor string as proof. Record host/OS identity and available pane/session identity from the invocation.

For historical claims without sufficient owner provenance, require explicit cleanup authority. Never pretend a reused pane proves ownership.

Expose ordinary authorized retirement through MCP. Keep the separately authorized cleanup override CLI-only unless the lead explicitly approves another surface.

### Historical request identity

Require a nonempty, valid request ID and a uniquely correlated historical claim.

An identified legacy claim may lack C1 checkout identity. Retirement can release that exact metadata claim because it removes no checkout. Preserve the absence of identity; do not discover a replacement identity from disk.

Refuse:

- Requestless records.
- Unknown IDs without a historical claim or matching retirement receipt.
- Ambiguous same-ID provenance.
- Changed expectations or conflicting dispositions.

Do not manufacture admissions or attach request IDs to legacy path-only holds.

### Durable retirement transaction

Use a dedicated origin orchestration owner, `src/preparation_retirement.rs`, and C3’s existing host disposition owner.

Persist an immutable retirement intent containing:

```text
operation identity
request and host correlation
exact expected historical claim
authority and actor
reason and recorded time
```

The host then:

```text
validates the original expectation
→ persists future-dispatch prohibition
→ releases only the matching preparation claim
→ persists the correlated retirement acknowledgement
```

Use C3’s guarded metadata-reconciliation path. When checkout identity exists, preserve its lifecycle-lock ordering. Missing or replaced filesystem paths never authorize discovery, teardown, or removal.

A retry uses the saved intent. It does not capture a new claim or rewrite actor/reason fields.

A conflicting existing non-retirement disposition is not overwritten. Return a conflict or the existing disposition’s status.

### Resource-only result

Successful output must state:

```json
{
  "resource": "preparation",
  "status": "retired",
  "outcome": "unknown",
  "future_dispatch": "prohibited",
  "ownership": "released",
  "task": "unresolved"
}
```

Include request ID, host, stable receipt identity, and audit metadata.

Before the host acknowledges, report retirement/release **pending**, not retired/released.

Retirement must not:

- Change the original preparation outcome or launch result.
- Create `NotStarted`, returned, stopped, or task-completion evidence.
- Close an item, pane, workspace, checkout, or branch.
- Terminalize a pending/running/uncertain Job.
- Release another request’s preparation or any independent live-work hold.

C5’s settlement logic must distinguish retirement from cancellation. A retirement fence or retired ticket alone cannot become no-launch proof.

Independent, genuine terminal evidence retains its existing meaning; retirement must neither manufacture nor erase it.

### Caller coverage

Cover public CLI, MCP, local host execution, remote disposition transport, duplicate delivery, service recovery, and the explicit authorized cleanup route.

For a host-local historical claim whose origin is unavailable, explicit cleanup authority may use the host-owned request fence. A later stale origin must encounter that fence before preparation or dispatch.

## Exact owned files

```text
src/preparation_retirement.rs         # new orchestration and authority owner
src/prepare.rs                       # C3 retirement disposition integration
src/work.rs                          # caller fence/recovery integration
src/unit_request.rs                  # prevent retirement-derived settlement
src/cli.rs
src/runtime.rs
src/main.rs
src/mcp.rs
tests/cli/preparation_retirement.rs   # new
tests/cli.rs                         # module registration
tests/cli/mcp.rs
```

Do not modify checkout removal, Job terminalization, branch deletion, retention, or provider task-state code.

## Red-before tests

| Named test | Setup, interleaving, assertions |
|---|---|
| `preparation_retirement::two_requests_on_one_path_retire_independently` | Establish A and B on one real checkout. Retire A. Assert B unchanged, checkout/pane retained, and cleanup still reports B’s hold. |
| `preparation_retirement::lost_ack_replays_the_same_retirement` | Real host persists fence/release; SSH discards acknowledgement. Restart and retry. Assert identical receipt/audit data and no repeated resource effects. |
| `preparation_retirement::retirement_and_dispatch_share_the_request_gate` | Test both lock orders with FIFO barriers: retirement first refuses late launch/wrapper; dispatch first preserves launched work and independent ownership. Neither order invents `not_started`. |
| `preparation_retirement::changed_claim_or_custody_refuses_without_rebinding` | Capture the expectation, block at the owning gate, then change claim provenance or custody revision. Release the gate. Assert conflict and unchanged claims. |
| `preparation_retirement::requestless_and_unknown_claims_refuse` | Table-test path-only legacy hold, empty ID, unknown ID, and ambiguous same-ID records. Assert no fence, claim deletion, or task-state change. |

Additional failure controls:

| Named test | Setup, interleaving, assertions |
|---|---|
| `preparation_retirement::replacement_checkout_is_never_touched` | Capture the historical claim, replace the filesystem checkout, then retire the old metadata claim. Assert no marker creation, teardown, removal, or mutation of the replacement. A replaced **stored claim** instead refuses. |
| `preparation_retirement::retired_resource_leaves_unit_unresolved` | Retire a unit-bound historical preparation; run recovery, observation, and settlement listing repeatedly. Assert the required output and no retirement-generated settlement. |
| `preparation_retirement::job_ownership_survives_preparation_retirement` | Seed valid C2 pending/running ownership for the same checkout. Retire preparation. Assert Job evidence unchanged and cleanup still refuses removal. |
| `preparation_retirement::unreachable_peer_reports_pending_not_released` | Persist authorized intent, fail transport, restart. Assert pending ownership status and unchanged host claim until a correlated acknowledgement arrives. |
| `preparation_retirement::cleanup_authority_is_explicit_and_audited` | Test owner gone with no authorization, explicit authorized cleanup, and duplicate replay. Assert refusal in the first case and stable actor/reason evidence in the others. |

## Acceptance

```sh
cargo test --test cli preparation_retirement::
cargo test --bin dogsled prepare::
cargo test --test cli unit_requests::
cargo test --test cli jobs::
cargo test --test cli mcp::
```

**Lead decision:** select the explicit cleanup authorization surface before dispatch. Recommended default: CLI-only cleanup mode with a required reason and recorded live actor identity; no automatic authorization from age or owner disappearance.

---

## Plan contradictions and decisions to resolve

| Finding | Evidence | Recommended resolution |
|---|---|---|
| The requested hardening section is numbered **6**, not 5. | `plans/lifecycle-cleanup.md:130–162` | Cite section 6; section 5 covers detached checkouts. |
| Literal no-new-capacity replay exceeds existing TTL-only reservation replay. | Plan `:158,240`; `src/node.rs:163–198` | Approve C4a’s durable grant receipt, or explicitly narrow the guarantee. |
| Successful recovery is impossible to prove for an incomplete host preparation receipt. | `src/prepare.rs:137–145,198–210`; existing regression at `tests/cli/launch_retry.rs:926–979` | Promise recovery of saved outcomes, not rerunning uncertain setup. |
| Cleanup retirement authority is required but its command-level form is unspecified. | Plan `:162,283` | Decide the explicit audited cleanup mode before C6. |
| Plan verification names Mise, but this revision’s repository gates use Moon. | Plan `:249,255`; `moon.yml:100–107`; `.moon/workspace.yml:9–13` | Update plan commands. Use focused Cargo tests per unit and the repository’s Moon integration gate. |

The configured commit hook runs:

```sh
env -u GIT_DIR moon run :#precommit --affected --status staged
```

The lead owns final integrated verification at the exact combined revision, including `moon run :check` and the CI-required web lint target.

**Next:** save this brief, resolve the capacity and cleanup-authority decisions, and bind C4a to the integrated C3b SHA.
