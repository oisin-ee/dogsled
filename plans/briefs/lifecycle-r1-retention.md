<!-- R1 PART 2 MUST FIX (found by D2, 2026-10-29): src/cleanup.rs remove_job, on explicit prune, deletes the terminal Job JSON, log and stop file once they are >24h old, while the service (src/jobs.rs) keeps records for 30 days. Required: prune deletes LOGS at 24h but keeps the terminal job record (and anything it references) for the same 30-day window and conclusive-terminal rule as the service; one owner, not two windows. Add a test: an explicit prune of a 2-day-old terminal job keeps the record and still reads "expired output", not unknown. -->
<!-- AR1 architect output (arch-ar1, 2026-10-28), reviewed main 24d620c and #205 at ff45c58.
Lead status: R1 is late in the chain (after C6). Before R1A dispatch the lead must:
  1. confirm #205 (final) keeps non-job tickets and does not delete sidecars needed by receipt replay;
  2. add a validity-extension unit (agent/placement/preparation stamping and checks) — new prerequisite outside R1 ownership;
  3. decide remote reference-release proof, expired-job identity retention, and the sweep::tell / custodian_of_event missing-item race (src/inbox.rs scope). -->

# AR1 — R1 retention contracts

**Cause:** Replay safety still depends on retained metadata, while request-age enforcement covers jobs only and several stores lack a durable completion timestamp.

**Status: contracts specified, but not ready for dispatch.** The prerequisite and ownership gaps below must close before deletion starts.

**Evidence:** Reviewed `origin/main` at `24d620cb18df3fc0b721a8407cd734649bb8e5fd` and PR #205 at `ff45c582c418cb1d3e9030be34e2a58e18e96ed2`.
The requested Mac checkout was unavailable. I read Git objects in the provided Linux checkout.
The architect restriction prohibited `git fetch`; `git ls-remote` confirmed that the local `origin/main` matched the remote.
No files or Git state changed. No tests ran. Unless marked **#205**, citations refer to the reviewed main revision.

## Binding prerequisites

1. **Land the corrected #205 implementation first.** Reuse its queue reader/writer and validity sidecar. Do not create another queue-expiry implementation.
   **Discrepancy:** the reviewed PR does **not** restrict expiry to job tickets. Its predicate checks terminal state and `created_ms`, without checking `Action::Job` (**#205 `src/work.rs:163–178`**).
   It can therefore remove an agent ticket that an unresolved admission still needs. Its sidecar deletion also needs review against retained-receipt replay: correlation reads the sidecar (**#205 `src/work.rs:197–206,535–555,585`**).
2. **Extend validity before deleting replay evidence.** Main explicitly bypasses non-job validity (`src/work.rs:114–137`).
   Required boundaries are listed below. These changes exceed the supplied R1 ownership lists.
3. **Define positive reference-release evidence.** A returned result, closed pane, acknowledged delivery, and completed task are different facts.
   `Prepared` retains origin, execution, settlement, custody acknowledgements, and reviews (`src/unit_request.rs:16–42`).
   Settlement acknowledgement follows provider settlement, but does not complete the task (`pi/unit-bridge.ts:161–173`; `pi/unit-contract.test.ts:150–158`).
4. **Add immutable completion timestamps where absent.** Proposed field: `finished_ms: Option<u64>`, written once by the store owner after a proven final transition.
   Tickets, admissions, preparation receipts, placement attempts, and launch progress currently lack it (`src/work.rs:81–99,841–853`; `src/prepare.rs:200–207`; `src/commands.rs:50–52`; `src/launch.rs:26–40`).
5. **Serialize R1A → R1B → R1C.** All three own `src/service.rs` and `tests/cli/retention.rs`.
   R1A registers the new test module in `tests/cli.rs`. Later units extend it without concurrent ownership.
   The lead must bind each dispatch to the full landed prerequisite SHA, not the reviewed main SHA.

### Validity extensions required before deletion

| Boundary | Required change and source |
|---|---|
| Origin issuance | Stamp agent and placement requests once, including prepared-unit requests. Currently only jobs receive a stamp (`src/runtime.rs:19–35`; `src/unit_request.rs:108–145`). |
| Queue/admission | Validate enqueue, immediate execution, retries, commit, and crash recovery. Preserve the original stamp when commit replaces the agent action (`src/work.rs:140–175,869–907`; `src/unit_request.rs:232–256,392–410`). |
| Placement | Validate before host selection and reservation. `Action::Place` bypasses `perform_locked`; host reservation accepts absent validity (`src/work.rs:869–899`; `src/commands.rs:56–150`; `src/node.rs:170–183`). |
| Preparation | Carry validity through local and remote preparation. Refuse recordless/unprovable calls before checkout/setup/pane effects. The current API carries only an optional request ID (`src/prepare.rs:56–113,120–158,455–464`; `src/cli.rs:549–557`). |
| Agent dispatch | Validate after correlated read-only replay, before allocation, and immediately before every native start attempt. Recheck after preparation and busy waits. Current dispatch has no age check (`src/work.rs:635–638`; `src/launch.rs:388–438,505–568,637–662,95–118`). |

**Additional prerequisite paths:** `src/runtime.rs`, `src/cli.rs`, `src/main.rs`, `src/hosts.rs`, `src/node.rs`, and the validity owner in `src/jobs.rs`.
Include transport/schema callers if the chosen validity envelope changes them.
`src/hosts.rs:527–560` and `src/main.rs:638–667` own the remote reservation/preparation boundaries.

Retain J2’s exact rules: 30-day execution window, five-minute future skew, immutable creation time, persistent high-water clock, and read-only correlated receipt replay.
Extend clock bootstrap to the new timestamp locations before deleting their evidence. It currently scans job-specific locations (`src/jobs.rs:77–108`).

## Shared deletion contract

Let `D = 2_592_000_000` milliseconds.
A record expires only when its owner proves completion, its valid completion timestamp satisfies `effective_now - finished_ms > D`, and no unresolved reader requires it.
Equality retains the record. Missing, corrupt, contradictory, or incorrectly ordered timestamps retain it and report the evidence gap.

Persist the high-water clock **before** deletion (`src/jobs.rs:163–182,371–374`).
Do not derive timestamps from IDs, file modification times, retry times, or launch confirmation.
Legacy records remain readable; missing validity never authorizes execution.

“Unreferenced” must exclude pending/admitting tickets, unresolved unit settlements/bindings, held preparations, active launch recovery, live matching execution, unsettled retained branches, and outstanding custody-dependent operations.
A pointer to a conclusively finished job is not live execution ownership.

Use owner-specific operations, not a generic directory sweeper.
The inbox supplies the ownership pattern, not its 24-hour duration or boundary (`src/inbox.rs:26,238–251,434–480`).

## R1A — Expire terminal admission metadata

**Deliverable:** Timer-driven expiry of terminal tickets, admissions, placement attempts, settled preparation claims, and preparation receipts without permitting replay.
**Owned paths:** `src/work.rs`, `src/commands.rs`, `src/prepare.rs`, `src/service.rs`, `src/unit_request.rs`, `tests/cli/retention.rs`, `tests/cli.rs`.
**Prerequisites:** Corrected #205, complete validity enforcement, and the reference-release contract.
**Generated paths:** None.

### Queue tickets

- **Store/owner/lock:** `$STATE/queue.json`, owned by `work`; `queue.lock`. Preserve #205’s format metadata and `save_queue` boundary (**#205 `src/work.rs:101–157`**).
- **Finished predicate:** `state ∈ {"admitted","failed","cancelled"}` plus a recorded final-transition timestamp. These transitions occur at `src/work.rs:234–245,1328–1352`. `created_ms` is not completion time. Retain pending/admitting and unresolved execution dependencies.
- **Readers:** Enqueue replay, admission restart, waiter, notification, unit commit/recovery/settlement, done ownership, and board (`src/work.rs:146–157,1287–1308,1392–1433`; `src/runtime.rs:76–105`; `src/unit_request.rs:276–308,392–410,430–496`; `src/done.rs:99–107`; `src/board.rs:817–822`).
- **Deletion order/refusal:** Persist `not_started` settlement before removing a failed/cancelled ticket. Make `receipt` and `recover` recognize saved terminal settlements without recreating a ticket. Then remove the row under `queue.lock`. Missing-ticket wait/cancel must refuse; board omission is safe.
- **Red-before tests:** Old creation with recent completion stays; exact 30-day boundary stays; terminal+1ms expires; pending/admitting stay; unresolved failed-unit ticket stays; settled ticket deletion cannot trigger re-enqueue; #205 job-sidecar behavior remains intact.

### Admissions and prepared-unit claims

- **Store/owner/lock:** `$STATE/admissions/<request>.json`, owned by `work` and `unit_request`; per-request lock. Unit preparation also uses `admissions/owner-<digest>.lock` (`src/work.rs:898–899`; `src/unit_request.rs:81–97,219`).
- **Finished predicate:** For units, a consistent `abandoned`, `stopped`, or `returned` settlement must exist. Returned settlements require their correlated `terminal_receipt`. Required binding/custody references must be released. `result: Some` proves launch, not execution completion (`src/unit_request.rs:276–307,417–424,691–868`).
- **Plain admissions:** Require positive final execution or preparation-failure evidence. `host.is_local() && launch.is_none() && error.is_some()` proves local preparation failure only (`src/work.rs:855–866`). A remote error or saved launch with unknown outcome is not completion proof.
- **Readers/deletion:** Commit/observe/abandon, recover, refresh/import, settlements/acks/reviews, legacy continuation, claim acceptance, sweep unit classification, and clock bootstrap depend on admissions (`src/unit_request.rs:151–171,203–410,528–556,894–1105`; `src/continuation.rs:369–437`; `src/claim.rs:201–210`; `src/sweep.rs:604–636`). Settle dependants first; delete admission last. Explicit operations on a missing admission must refuse, never become unmanaged execution.
- **Red-before tests:** Old uncommitted, committed-unreturned, unacknowledged, and custody-dependent records survive. Fully released terminal records expire. Commit/observe/accept after deletion refuse. Recovery cannot mint another ticket. Same-origin preparation cannot silently refresh an expired execution identity.

### Preparation holds

- **Store/owner/lock:** `$STATE/preparations.json`, owned by `prepare`; `preparations.lock` (`src/prepare.rs:436–450,501–510`).
- **Finished predicate:** Current settlement proof is `launches.consumed(request_id,path) || preparation_failed(request_id)` (`src/prepare.rs:479–495`). Persist that settlement before deleting the launch/admission evidence that proves it. Add a write-once settlement timestamp; `Pending` currently has none (`src/prepare.rs:25–30`).
- **Readers:** Cleanup/close protection reads `pending_trees`; launch and placement release matching holds (`src/cleanup.rs:1528–1537`; `src/work.rs:635–638,678–679,878–892`). No direct custody, inbox, or board reader exists for this store.
- **Deletion order/refusal:** Settle the exact request/path under its lock, then expire the settled row. Never clear another request’s hold for the same path. Delete the proving admission/launch only afterward. Missing proof must preserve a hold, not make cleanup eligible.
- **Red-before tests:** Interrupted and unidentified legacy holds stay; two requests sharing a path remain isolated; settled rows expire; removing their old proof cannot make a settled hold become pending again.

### Preparation receipts

- **Store/owner/lock:** `$STATE/preparation-receipts/<request>.json`, owned by `prepare`; `<request>.lock` (`src/prepare.rs:132–146`).
- **Finished predicate:** Exactly one of `prepared` or `failure` is present. Both present is contradictory; neither is uncertain (`src/prepare.rs:196–221`). Require the new completion timestamp and no remaining preparation/admission/launch dependency.
- **Readers:** Local and remote preparation replay; admission retries depend indirectly on it (`src/prepare.rs:140–146,455–464`; `src/work.rs:931–975`). Cleanup uses the separate hold, not this receipt.
- **Deletion order/refusal:** Resolve its hold and dependencies before deleting the receipt under its request lock. After deletion, stale or missing-validity preparation must refuse before setup, worktree, or pane effects. Retained correlated outcomes remain read-only.
- **Red-before tests:** Success/failure expire; uncertain/contradictory receipts stay; held success stays; replay after deletion performs zero preparation effects; retained expired receipt replays without effects.

### Placement attempts

- **Store/owner/lock:** `$STATE/placement-attempts/<request>.json`, owned by `commands`; `<request>.lock` (`src/commands.rs:50–82`).
- **Finished predicate:** No existing field proves completion: the record contains only `host`, or JSON `null`. Neither host selection nor reservation TTL is execution completion. Add an explicit terminal timestamp only after the corresponding request settles.
- **Readers:** Placement retries use it to preserve host affinity; preparation/admission depend indirectly on that affinity (`src/commands.rs:83–95,137–168`). No direct unit-return, custody, inbox, or board reader exists.
- **Deletion order/refusal:** Stop all retry/recovery paths through immutable validity, resolve dependent preparation, then remove the selection. Host reservation expiry remains a separate owner operation (`src/node.rs:162–183`). Missing selection must not let an expired request choose a new host.
- **Red-before tests:** Pending/uncertain selections stay; terminal selections expire; same-ID retry after expiry cannot reserve locally or remotely; ambiguous transport never migrates a preparation to another host.

## R1B — Expire finished items and closed marks

**Deliverable:** Remove finished presentation/history metadata without losing recovery identity, unresolved task evidence, or branch custody.
**Owned paths:** `src/items.rs`, `src/done.rs`, `src/service.rs`, `src/sweep.rs`, `tests/cli/retention.rs`, `tests/cli/done.rs`.
**Prerequisites:** R1A plus resolution of the job-expiry identity and notification races below.
**Generated paths:** None.

### Items

- **Store/owner/lock:** `$STATE/items.json`, owned by `items`; `items.lock` (`src/items.rs:87–92,142–171`).
- **Finished predicate:** `closed_ms.is_some()` with proven closure, no unresolved execution/binding, and `branch.is_none() || branch.landed || dropped_ms.is_some()`. Retain identities required by recovery/custody references. Do **not** use `items::finished`: it selects **open** agent items (`src/items.rs:257–264`). Closure alone can come from a missing-pane observation (`src/items.rs:302–320`).
- **Readers:** Read/close/log routing; handoff/reply delivery; unit observation; custody claim; branch drop; cleanup/gate; watchdog; board; job-expiry detection (`src/runtime.rs:282–325,516–539`; `src/sweep.rs:230–285,485–517`; `src/unit_request.rs:327–338`; `src/custody.rs:393–405`; `src/drop.rs:46–79`; `src/gate.rs:46–65`; `src/watchdog.rs:162`; `src/board.rs:800–822`; `src/jobs.rs:491–499`).
- **Deletion order/refusal:** Resolve branch and unit dependencies first. Quiesce child-event delivery, then remove the item under custody-compatible locking. Missing identity must refuse claim/drop/recovery operations. Board history may disappear. Do not reinterpret an expired child as another agent with a reused name or pane.
- **Red-before tests:** Closed landed/dropped items expire; open, unreturned, unlanded, and recovery-referenced items stay; custody claim versus expiry serializes; no duplicate handoff import; no notification reaches a reused owner session; local and remote expired-job reads preserve their required result.

**Blocking identity dependency:** C8 uses the item to distinguish expired jobs from unknown jobs and route remote reads (`src/jobs.rs:491–499`; `src/runtime.rs:310–323,533–564`).
Deleting that item removes this distinction. Define the replacement/recovery-reference lifetime before enabling job-item deletion. Do not invent permanent tombstones.

**Blocking notification race:** `sweep::tell` copies items before delivery. If retention removes the item meanwhile, `inbox::custodian_of_event` falls back to the owner pane with no session (`src/sweep.rs:487–515`; `src/inbox.rs:1143–1173`).
Fix the missing-child-event behavior in its owner, `src/inbox.rs`, or provide an equivalent serialized identity contract. This requires a scope extension.

### Done marks

- **Store/owner/lock:** `$STATE/done.json`, owned by `done`; `done.lock`. Handoff operations additionally use `handoff.lock` (`src/done.rs:10,184,649–650`).
- **Finished predicate:** A closed, sealed handoff with preserved reply evidence, settled branch, and no matching live execution or unresolved unit/recovery reference. Use `closed_ms`, not `marked_ms` or `sealed` as a timestamp (`src/done.rs:30–60,145–164,733–743`).
- **Readers:** Done observation/input rejection, legacy continuation, host snapshot/handoff import, sweep, inbox target resolution, cleanup branch protection, and branch disposal (`src/done.rs:183–237,542–643`; `src/continuation.rs:65–68`; `src/node.rs:94`; `src/items.rs:221–250`; `src/inbox.rs:527–557`; `src/cleanup.rs:1106–1124`; `src/drop.rs:283–289`).
- **Deletion order/refusal:** Persist required owner-side handoff/branch identity first. Remove only closed, unreferenced marks under `done.lock`, ordered with handoff. Never delete a mark while it still prevents inbox input or protects an unlanded branch.
- **Red-before tests:** Closed settled marks expire; marked-but-unsealed, unclosed, live-session, unreturned, unlanded, and recovery-referenced marks stay; stale handoff saves cannot resurrect deleted marks; concurrent input cannot reopen a finished execution.

## R1C — Expire launch evidence and owned transcripts

**Deliverable:** Remove finished, unreferenced launch families without enabling stale dispatch or losing terminal-return evidence.
**Owned paths:** `src/launch.rs`, `src/launch/recovery.rs`, `src/unit_reply.rs`, `src/service.rs`, `tests/cli/retention.rs`, `tests/cli/launch_retry.rs`, `tests/cli/transcripts.rs`.
**Prerequisites:** R1A, R1B, complete agent-dispatch validity, and positive remote reference-release evidence.
**Generated paths:** None.

### Launch receipts, progress, and seeds

- **Store/owner/lock:** `$STATE/launches/<id>.json`, `<id>.progress.json`, and `<id>.unit.json`. Agent launch ownership belongs to `launch`; job receipt ownership belongs to `work`. Use `launches/<id>.lock`; pane recovery also uses `launch-panes/<digest>.lock` (`src/launch.rs:20–22,388–438,759–775`; `src/work.rs:640–680`).
- **Finished predicate:** `Receipt::Started` and `Progress.confirmed` prove startup only. Require a consistent final refusal, correlated terminal return, or authoritative stopped/closed execution evidence, plus no live/resource/reference dependency (`src/launch.rs:141–147,442–467,812–887,945–1035`). An uncertain attempted launch remains protected.
- **Job receipts:** Require execution-host terminal evidence, not receipt age or job-record absence. C8 removes the Job record without copying `ended_ms` into its launch receipt (`src/work.rs:603–607`; `src/jobs.rs:403–423`). Preserve the necessary terminal fact before that deletion; never extend the Job retention window.
- **Readers:** Agent/job replay; recovery surveys and close; preparation consumption; done identity/input/handoff; unit terminal acknowledgement and peer refresh; continuation; gate ownership; node lifecycle; board docking (`src/launch/recovery.rs:37–47,145–235,241–345`; `src/done.rs:69–76,139–164`; `src/launch.rs:236–328,812–1035`; `src/continuation.rs:54–68`; `src/gate.rs:48`; `src/node.rs:134`; `src/board.rs:274`).
- **Deletion order/refusal:** Release dependent holds and marks first. Under the request lock, delete expendable payloads, result/continuation receipts, then seed and progress last. Any interrupted deletion must remain fail-closed. Missing execution evidence must not become `Plain`, cleanup permission, successful return, or permission to allocate again (`src/launch.rs:268–288,954–957`).

**Red-before tests:** Final unreferenced families expire; confirmed live launches stay; uncertain/mismatched records stay; stale host dispatch after complete deletion has zero effects; retained correlated replay stays read-only; restart after each deletion boundary cannot start again; remote unresolved return evidence survives.

### Session transcripts and launch payloads

- **Store/owner/lock:** `$STATE/launches/<id>.sessions/*.jsonl`, `<id>.task.md`, and `<id>.contract.md`. `launch` owns lifecycle; `unit_reply` validates transcript reads. Use the launch request lock (`src/launch.rs:759–807`; `src/unit_reply.rs:66–113`; `src/work.rs:702–727`).
- **Finished predicate:** The enclosing execution satisfies launch-family completion and reference release. A terminal receipt alone does not prove the worker process stopped. Never age files independently.
- **Readers:** Worker startup reads task/contract/seed paths. Terminal acknowledgement reads the exact session transcript before persisting its immutable reply. Handoff then reads saved reply evidence (`src/launch.rs:637–658,880–886,382–385`). Custody, inbox, and board consume the resulting evidence indirectly.
- **Deletion order/refusal:** Persist the correlated terminal receipt and required recovery evidence before deleting transcripts. Delete only this validated execution directory, without following symlinks or touching harness-global sessions. A missing transcript before capture must refuse, not synthesize a result.
- **Red-before tests:** Terminal acknowledgement races with retention safely; live/uncertain sessions stay; captured, stopped, unreferenced sessions expire; malformed/outside/symlink paths remain untouched; unrelated Claude/Codex/Pi sessions remain untouched.

### Continuation receipts

- **Store/owner/lock:** `$STATE/launches/<id>.continuations.json`, owned by `continuation`, under the launch request lock (`src/continuation.rs:138–155`).
- **Finished predicate:** `phase == Accepted` proves input persistence, not execution completion. `Prepared` remains uncertain (`src/continuation.rs:334–341`). Expire only with the conclusively finished, unreferenced enclosing execution.
- **Readers:** Legacy continuation replay uses these receipts after inbox records expire (`src/continuation.rs:14–18,94–128`). Done/input and admission identity are prerequisites; board has no direct reader.
- **Deletion order/refusal:** Keep continuation replay evidence until continuation becomes impossible. Remove it before the enclosing launch identity disappears, under the same lock. Missing launch/admission must refuse continuation.
- **Red-before tests:** A live execution retains accepted receipts beyond inbox expiry; prepared receipts retain uncertain input; finished families expire; replay after deletion cannot deliver another message or continue a replacement mark.

## Concurrency requirements

- **Preserve existing lock direction:** legacy continuation takes `admissions/<id> → done → launches/<id> → inbox/outbox` (`src/continuation.rs:9–12`). Launch recovery takes `launches/<id> → launch-panes/<digest>` (`src/launch/recovery.rs:267–269,325–326`). Never acquire another request lock inside these launch locks.
- **Coordinate admission and custody:** settlement acknowledgements take `admissions/<id> → custody`; imported replies take admission then items (`src/unit_request.rs:824–887,983–1043`). Item expiry must serialize with custody claims and re-read under lock, not trust a pre-lock reference snapshot (`src/custody.rs:348–405`).
- **Coordinate preparation publication:** unit preparation writes under the owner-hash lock, not the request lock (`src/unit_request.rs:81–145`). An expiry pass using only the latter does not serialize same-origin discovery.
- **Do not rely on service-lane order:** lanes run on separate threads (`src/service.rs:164–175`). Retention must explicitly order dependent owner operations. Persist/release the clock lock before the deletion walk.
- **Never unlink lock files or enclosing lock directories.** `Ledger::with_lock` locks an opened file (`src/ledger.rs:76–104`). Unlinking it permits another process to lock a replacement inode. Preserve queue, admission, owner-hash, preparations, preparation-receipt, placement-attempt, items, done, handoff, custody, launches, launch-panes, job, validity-sidecar, and clock locks.

## Acceptance and remaining decisions

**Test method:** Add deterministic real-CLI tests using the private `Rig`, injected clock, and `serve --once --interval 1` (`tests/cli.rs:1`; `tests/cli/jobs.rs:34–47`).
For each new expiry assertion, record failure on the prerequisite base and success on the implementation revision.
A successful service exit alone is insufficient: assert persisted records, refused replay, and absence of side effects.

| Unit | Scoped acceptance commands |
|---|---|
| R1A | `cargo test --test cli retention::r1a_`; `cargo test --test cli request_validity::`; `cargo test --test cli unit_requests::`; `cargo test --test cli placement::` |
| R1B | `cargo test --test cli retention::r1b_`; `cargo test --test cli done::`; `cargo test --test cli sweep::`; `cargo test --test cli jobs::` |
| R1C | `cargo test --test cli retention::r1c_`; `cargo test --test cli launch_retry::`; `cargo test --test cli transcripts::`; `cargo test --test cli unit_seeded::` |

The lead owns the final repository gate defined by `moon.yml:check`. Writers retain normal commit hooks.
This brief authorizes no push, PR, merge, deployment, dependency change, or installed-version change.

**Unresolved decisions requiring lead disposition:**
- The host currently lacks proof that a remote origin released its unit/recovery references. `Progress` contains no such acknowledgement (`src/launch.rs:26–40`). Specify that proof before remote seeded-launch deletion.
- Specify the retained identity contract for expired job reads and outstanding recovery references before item deletion.
- Approve prerequisite ownership outside the supplied R1 paths, including validity transport, terminal-proof transfer, and missing-child notification handling.

**Next action:** Correct #205’s ticket scope, then assign the validity prerequisite before dispatching R1A.