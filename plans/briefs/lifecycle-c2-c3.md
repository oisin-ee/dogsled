<!-- C2/C3 architect output (arch-c2c3, 2026-10-28), reviewed C1 head c370255 + main b51ba59.
Lead decisions: ALL recommended defaults in the C2 and C3 decision tables are APPROVED. Split C3 into C3a then C3b, serialized after C2.
Writer base for C2: origin/main after #213 (C1) merges. Writers MAY push their own branch (lead lands via PR) — this overrides the "no push" line below. -->

# C2 and C3 writer contracts

**Cause:** C1 protects checkout operations, but Job records do not retain checkout identity, and preparation release deletes claims without a durable dispatch fence.

## Reviewed revision and dispatch gate

All source references below use **C1 head `c3702552d0ea1f7790fb745cd7f0509e4459ea4f`**, verified through GitHub PR #213.

The reviewed main revision was `b51ba5968741da7ff19174affaf88ebf5257370e`. It is **not an ancestor of C1 head**. Before dispatch, the lead must supply the committed integration revision containing both. Do not use C1 head alone and lose newer main changes.

Git-state changes were prohibited by the architect profile, so I used GitHub API/raw branch contents instead of fetching. No files changed and no tests ran. The red tests below are specifications, not claimed test results.

## Shared contract

- Preserve C1’s identity: **canonical common directory + administrative directory + generation**, not checkout path alone (`src/worktree.rs:26–34`).
- Use `Guard::observe` for historical ownership. It must not create a replacement’s generation marker (`src/worktree.rs:50–65`).
- Keep `prepare::used` a short store operation. Never add repository or preparation-receipt acquisition inside it.
- No queue lock across admission locks or RPCs. Preserve continuation’s `done → launches` order (`src/continuation.rs:65–67`).
- These assignments authorize scoped implementation and commits through required hooks. They do **not** authorize pushes, PRs, merges, installation, deployment, or changes to existing host state.

---

# C2 — Preserve job ownership through process startup

**Prerequisite:** integrated C1 and current main.  
**Suggested branch:** `feat/job-startup-ownership`.  
**Size target:** approximately 800–1,200 changed lines, including tests and fixture updates.

**Deliverable:** A published job claim protects its checkout before the process exists; a delayed wrapper cannot execute in a removed or replacement checkout.

## Current owning paths

| Path at C1 head | Relevant behavior |
|---|---|
| `src/work.rs:682–760` | Publishes claiming intent, allocates a pane, publishes pending claim, then dispatches the wrapper. |
| `src/work.rs:849–892`, `896–949` | C1 validates checkout ownership under the repository lock, but puts checkout identity in the **launch result**, not the Job claim. |
| `src/jobs.rs:254–276`, `806–840`, `878–954` | Job has no checkout identity. Claim publication and startup compare execution fields and validity. |
| `src/jobs.rs:958–981`, `619–636` | Wrapper holds lifetime `jobs/<id>-exec`, marks Working, reports the pane, checks validity, and spawns without repository protection. |
| `src/cleanup.rs:1519–1599`, `1603–1644`, `1767–1845` | Shared close/prune holds consider preparations, launches, panes, agents and processes, but not every nonterminal Job record. |

C1’s repository mutex is `src/worktree.rs:111–122`. Do not acquire the lifetime exec lock from `work::launch`.

## Concrete race windows

| Race | Interleaving |
|---|---|
| Dispatch wins; wrapper has not started | Launch publishes pending Job → Herdr accepts command → launch receipt is saved and preparation released → cleanup sees no process or active pane → checkout disappears → wrapper starts. |
| Removal wins before dispatch | Removal completes → a stale job launch still names the missing cwd → `work::launch` takes its non-repository fallback or receives no checkout from discovery → it publishes and dispatches a claim anyway. |
| Replacement between dispatch and execution | Job is pending → original checkout disappears → another checkout occupies the same path → wrapper validates command/cwd strings → command executes in the replacement. |
| Startup report blocks | Wrapper marks Working → pane-report call blocks → removal runs before `Command::spawn` → current code rechecks only request age. |
| Ticket failure masks live ownership | Dispatch outcome is ambiguous → ticket fails → pending, uncertain, or started Job remains → cleanup lacks an independent Job hold. |

## Binding implementation design

### Persist ownership in the existing Job

Add a versioned startup-ownership field to `Job`. Represent these cases explicitly:

```text
Git checkout: captured cleanup::Tree with C1 Identity
Non-repository: explicitly recorded execution scope
Missing/unsupported evidence: readable, but not executable
```

Do not let an absent field mean “non-repository.”

Copy the guarded checkout into the **claiming intent before pane allocation**, then preserve it through pending, Working, uncertain, and terminal records. Include it in `publish_claim` and `pending_claim` equality checks.

Continue publishing the checkout in the launch result. That result is not the wrapper’s startup authority.

For stale startup, missing cwd must refuse before allocation. A request already bound to Git must never fall back to non-repository execution. Preserve a known pre-lock identity across the wait; compare it after acquiring the repository lock.

### Startup locking

Use these acquisition orders, skipping unused levels:

```text
Host launch:
preparation-receipts/<id>
→ repository lifecycle
→ launches/<id>
→ short Job/store locks

Wrapper startup:
preparation-receipts/<id>
→ repository lifecycle
→ nonblocking jobs/<id>-exec acquisition
→ short jobs/<id>
→ validity clock
```

The exec lock remains a **lifetime lock**, but acquire it nonblockingly while holding startup gates. A duplicate wrapper refuses immediately; it must not hold the repository mutex while waiting for another job’s lifetime.

Provide a small RAII lock handle through the existing Ledger implementation. Preserve its explicit-unlock semantics (`src/ledger.rs:76–105`). Do not create a second lock framework.

Refactor capture into startup and monitoring phases:

1. Under startup gates, reload and validate the claim and captured checkout.
2. Preserve the existing pane report and post-report validity check.
3. Immediately before opening output/spawning, revalidate checkout identity and execution authority.
4. Spawn while startup gates remain held.
5. Release preparation/repository gates; retain only the exec lock during process monitoring.

Once those outer gates are released, the wrapper must not reacquire them while holding exec.

C2 checks existing claim and validity fences. C3 adds disposition-fence validation at this same startup boundary.

### Cleanup ownership

Add one Job-owned predicate for conclusively terminal records. Reuse the retention interpretation rather than `Ownership::Started`, whose current classification checks `started_ms` first (`src/jobs.rs:756–777`).

Every other state holds the relevant checkout: claiming, pending, Working, uncertain, unknown, or contradictory evidence.

Use that predicate in the shared close/prune hold path. Reload Job evidence during C1’s final protected ownership check, not only during the initial survey.

For legacy records without identity, conservatively hold matching cwd/path descendants. Corrupt evidence must refuse removal, not look absent. Failed tickets do not alter this rule.

## Owned files

| Ownership | Paths |
|---|---|
| Implementation | `src/jobs.rs`, `src/work.rs`, `src/cleanup.rs`, `src/ledger.rs` |
| Behavior tests | `tests/cli/jobs.rs` |
| Shared fixture support | `tests/cli.rs`, limited to executable-claim fixtures and reusable synchronization support |
| Mechanical struct-literal updates only | `src/board.rs`, `src/work_tests.rs` |

No preparation-disposition schema, cancellation, placement-resume, or retirement implementation belongs in C2.

## Red-before tests

Use `Fifo`, bounded child-process helpers, and actual kernel locks from existing fixtures. Prior art: `tests/cli.rs:344–399`, `tests/cli/launch_retry.rs:5–33`. No scheduling sleeps.

| Test name | Setup, barrier, and assertions |
|---|---|
| `dispatch_before_removal_keeps_pending_job_checkout` | Create a clean managed checkout. Fake Herdr captures the wrapper command and returns success without executing it. After host launch returns, hide incidental pane/process holds. Apply a captured prune plan and attempt close. Assert checkout remains, teardown never ran, and refusal identifies the Job. Then execute the captured wrapper once. |
| `removal_before_dispatch_refuses_stale_job_startup` | Capture a job request for a managed checkout. Complete removal first; use teardown/removal acknowledgements to establish the order. Submit the stale launch. Assert refusal before Job publication, pane allocation, dispatch, or launch receipt creation. |
| `exec_refuses_replacement_checkout_at_same_path` | Obtain a real pending claim and delay its wrapper. Replace the checkout using fixture Git operations; recreate the same branch/path, including a case reusing the administrative-directory basename. Execute the saved wrapper. Assert no command marker, no log, no terminal result, no replacement mutation, and no generation creation during refusal. |
| `exec_waits_for_repository_gate_before_starting` | Hold the common-directory lifecycle lock. Start a wrapper with valid captured ownership. Use the Git/FIFO rendezvous to prove arrival, then let its bounded lock budget expire. Assert lifecycle-lock refusal, unchanged pending record, no log or command. After unlocking, the same pending claim can execute once. |
| `nonterminal_job_holds_survive_failed_ticket_and_missing_pane` | Table-test claiming, pending, Working, uncertain, unknown state, and legacy identity-less records. Give each a failed ticket and no visible pane/process. Assert close/prune preserve the checkout. Include conclusively terminal records as a control that no longer supplies a Job hold. |

Also retain the current dispatch-failure tests. Add a post-pane-report identity-change case to the replacement test using a FIFO-controlled report shim.

## Acceptance and decisions

**Writer acceptance:**

```bash
cargo test --test cli jobs::
```

Run each new test red before implementation, then run the suite once at the final revision. The lead should also run `launch_retry::` and `request_validity::` after integration.

| Decision/risk | Recommended default |
|---|---|
| Historical pending claims lack checkout evidence | Refuse new execution; retain readable records and cleanup holds. Never manufacture ownership from today’s path. |
| Duplicate wrapper contends for exec lock | Immediate refusal, not bounded waiting under repository protection. |
| Failed spawn or startup validation | Preserve current uncertainty/terminal semantics. Do not invent `not_started` or permission to retry. |
| Repository serialization during execution | Hold only through spawn, never through command runtime. Test this with a running command blocked on a FIFO. |
| Additional constructor/fixture fallout | Permit mechanical updates within declared fixture paths; return to the lead for behavioral scope expansion. |

---

# C3 — Version preparation disposition and enforce its fence

**Prerequisite:** accepted C2.  
**Split:** C3a owns host disposition and enforcement; C3b owns correlated transport and remaining caller integration.

**Cause:** Preparation release currently removes a path/request row, while immutable preparation replay and several dispatch paths can still use that request without a durable disposition check.

## Current owning paths

| Path at C1 head | Relevant behavior |
|---|---|
| `src/prepare.rs:109–211`, `233–244` | Request-keyed preparation receipt and replay; preparation receipt lock precedes repository lock. |
| `src/prepare.rs:339–377`, `445–476`, `480–560` | Records pending ownership; error cleanup calls `used`; `used` deletes by path/request ID without a disposition receipt. |
| `src/prepare.rs:512–530`; `src/work.rs:1128–1137` | Pending-hold assessment can infer settlement from launch survey or a local admission error. |
| `src/work.rs:902–949`, `1140–1170`; `src/main.rs:676–678` | Successful launches release locally; placement uses raw `host prepared`; host returns an uncorrelated `{"prepared":true}`. |
| `src/launch.rs:1075–1137`; `src/continuation.rs:65–67` | Unit refusal releases under the launch lock; continuation already holds admission, then done, then launch. Both constrain new gate placement. |

`Survey::consumed` also accepts request/path settlement evidence, including lost-pane cases (`src/launch/recovery.rs:164–177`, `222–229`). Do not treat that inference as a persisted fence.

## Concrete race windows

| Race | Interleaving |
|---|---|
| Release before delayed launch | `host prepared` removes the hold → delayed launch arrives → checkout still exists → launch proceeds without any release fence. |
| Release before delayed preparation replay | Release removes the pending row → original successful preparation receipt remains → same-request prepare returns its old pane/path → caller tries to use it again. |
| Wrong-incarnation release | Caller retained path/request ID → checkout or pending identity changed → `used` matches only path/ID and deletes the new record. |
| Interrupted disposition | Fence/release work partially completes → host dies or acknowledgement disappears → caller cannot distinguish held ownership, released ownership, or another request’s acknowledgement. |
| Lock inversion | Wrapper or cleanup holds repository protection → release helper tries to acquire preparation receipt; another preparation already holds that receipt and waits for repository protection. Moving outer locks into `used` creates this cycle. |

## Shared disposition protocol

Store a versioned receipt beside the existing outcome:

```text
preparation-receipts/<request-id>.json
preparation-receipts/<request-id>.disposition.json
```

Use the **disposition receipt itself as the durable fence**. Do not introduce a second file containing competing fence authority.

Its typed contents must include:

| Field | Required meaning |
|---|---|
| Correlation | Protocol version, request ID, immutable preparation request, captured checkout identity, and relevant prepared pane/terminal identity |
| Disposition | Transfer to the exact saved launch owner, or release with an explicit validated basis |
| Future use | Only the already-published matching owner may proceed, or future startup is prohibited |
| Outcome | Separate execution evidence from ownership release; preserve unknown/possibly-executed outcomes |
| Progress | Fence persisted, then matching claim released; acknowledgement repeats the exact correlation and released state |

A successful dispatch transfers preparation ownership to the existing Job/launch. It **must not prohibit that pending Job’s first wrapper startup**.

A prohibition fence cannot be overridden by a successful old preparation receipt. A mismatched or unsupported receipt refuses without mutation.

Host transaction:

```text
validate request and immutable host evidence
→ persist fence
→ release only the exact matching claim
→ persist released disposition
→ return correlated acknowledgement
```

An identical retry resumes this transaction. A conflicting retry never changes it. If the claim is already absent, only an existing matching disposition can prove that this transaction released it.

---

## C3a — Host disposition owner and startup enforcement

**Suggested branch:** `feat/preparation-dispositions`.  
**Base:** exact accepted C2 integration SHA, supplied by the lead.  
**Size target:** approximately 900–1,200 changed lines.

**Deliverable:** Every host-side preparation release has durable, identity-correlated disposition evidence, and subsequent startup respects it.

### Writer edits

1. **Give `prepare` ownership of disposition policy.** Add typed request/receipt/acknowledgement structures, preferably under `src/prepare/disposition.rs`. Capture sufficient identity in the immutable preparation outcome to survive pending-row removal. Do not derive historical identity from a current path.

2. **Make release an exact-match leaf.** Keep the actual pending-row removal short. Require the caller’s validated captured claim, not only path/ID. Never acquire outer locks there. For preparation failure, persist the failure outcome before releasing ownership; interruption before that point remains held.

3. **Gate host use and release.** Acquire preparation receipt before repository protection. Validate fences in `prepare::local`, `work::launch`, and C2’s wrapper startup boundary. Keep final launch-receipt replay read-only. A matching transferred owner may finish its existing startup; a prohibited or mismatched request cannot allocate, dispatch, or recreate ownership.

4. **Migrate current host release sites.** Cover successful job/agent launch, returned preparation failure, local placement release, `HostCommand::Prepared`, and correlated unit refusal. For `unit_receipt`, either enter the outer gates before the launch lock or release the launch lock and revalidate under the complete ordered transaction. Never acquire preparation/repository locks inside its current launch-locked block.

5. **Stop inference from replacing disposition evidence.** A failed admission/ticket or pane disappearance alone cannot release a preparation. Unresolved historical records stay held unless the host can produce the matching disposition from authoritative evidence. Requestless holds remain unsupported for release.

For identity-bearing checkouts, use:

```text
preparation-receipts/<id>
→ repository lifecycle
→ launches/<id>, when launch proof is required
→ short preparation/Job stores
```

Reuse C1’s verified-absence reconciliation. A replacement or contradictory record is not verified absence.

### Owned files

| Scope | Paths |
|---|---|
| Disposition owner | `src/prepare.rs`, new `src/prepare/disposition.rs` |
| Host enforcement/release | Host-launch portions of `src/work.rs`; startup portion of `src/jobs.rs`; refusal-release portion of `src/launch.rs`; `HostCommand::Prepared` routing in `src/main.rs` |
| Tests | Preparation unit tests; disposition/fence additions to `tests/cli/jobs.rs` and `tests/cli/launch_retry.rs` |

C3a may keep the existing identified `host prepared --path --request-id` input temporarily, but the host must validate it against its immutable preparation and return a typed acknowledgement. It must no longer call raw `used`. C3b replaces the caller’s weak correlation with the full versioned request.

### Red-before tests

| Test name | Setup, barrier, and assertions |
|---|---|
| `disposition_releases_only_the_matching_preparation_claim` | Prepare requests A and B against one checkout. Release A using its captured binding. Assert B remains byte-identical; outcome receipt stays immutable; A has a correlated released disposition. |
| `disposition_mismatch_never_mutates_ownership` | Table-test wrong request, preparation tuple, generation, pane/terminal, version, and contradictory existing disposition. Assert all relevant files remain byte-identical and no Git/Herdr mutation occurs. |
| `prohibited_preparation_cannot_replay_into_new_startup` | Complete preparation, persist a valid prohibition disposition, then invoke same-request preparation and host launch. Assert no setup, allocation, dispatch, claim recreation, or generation creation. Existing APIs ignore such a sidecar before implementation. |
| `disposition_recovers_fence_before_release_and_release_before_ack` | Use debug CLI crash points immediately after fence persistence and after matching-row removal. Restart and repeat the identical release. Assert one immutable disposition, preservation of unrelated claims, and identical acknowledgements. No sleeps. |
| `transferred_preparation_allows_only_its_existing_job_owner` | Delay a real pending wrapper while launch transfers preparation ownership. Assert that wrapper executes once. Mutate the copied claim’s owner/identity in separate cases and assert refusal. An unrelated request cannot inherit the transfer. |

For the new disposition API, establish the typed seam before behavior implementation, then demonstrate assertion failures. Do not present “unknown CLI argument” as the only red evidence.

### Acceptance

```bash
cargo test --bin dogsled prepare::
cargo test --test cli jobs::
cargo test --test cli launch_retry::
```

Keep the negative control below unchanged.

---

## C3b — Correlated transport and caller fence integration

**Suggested branch:** `feat/preparation-fence-routing`.  
**Base:** exact accepted C3a SHA.  
**Size target:** approximately 700–1,100 changed lines.

**Deliverable:** Callers send the exact disposition request, accept only its acknowledgement, and cannot bypass the fence through old peer or continuation paths.

### Writer edits

1. **Transport the binding.** Carry the host-produced preparation binding through `Prepared` and the saved launch/admission data that needs it. Add versioned disposition input to the internal host command. Do not change unit execution IDs, seeds, terminal acknowledgements, or task-binding semantics.

2. **Validate acknowledgements completely.** Replace the discarded remote `Value` at `src/work.rs:1154–1163`. Require matching version, request, preparation binding, disposition, and released state. Empty objects and `{"prepared":true}` are not acknowledgements.

3. **Fail closed on unsupported peers.** Use an explicit required protocol argument at mutation boundaries, following the existing job-validity transport pattern. Never retry by stripping the new argument. Retain unresolved ownership when the peer is unreachable or cannot prove its response.

4. **Order continuation correctly.** Where continuation can reuse the affected execution, enter preparation/repository gates before `done → launches`. A matching transferred agent execution may continue under existing rules. A prohibition fence or changed checkout may not expose new input.

5. **Preserve existing recovery semantics.** Keep successful launch receipt replay read-only. A failed ticket alone never becomes `not_started`. Retry only the exact disposition transaction here; do not add placement resume, cancellation intent, retirement commands, or new Attempt phases.

C4 still owns result-before-release placement replay. C3 must not claim to solve a lost placement result by merely acknowledging release.

### Owned files

| Scope | Paths |
|---|---|
| Wire and routing | `src/cli.rs`, `src/main.rs`, transport portions of `src/prepare.rs` and `src/work.rs` |
| Continuation | `src/continuation.rs` |
| Tests | `tests/cli/launch_retry.rs`, `tests/cli/legacy_continue.rs`, `tests/cli/unit_requests.rs` |
| Mechanical literal updates if binding fields require them | `src/work_tests.rs`, `src/unit_reply.rs` |

No Pi implementation change is expected. `mise run check:pi` verifies that the existing execution and terminal contracts remain compatible.

### Red-before tests

| Test name | Setup, barrier, and assertions |
|---|---|
| `remote_preparation_release_requires_correlated_ack` | Fake peer returns `{}`, legacy boolean ack, wrong request, wrong identity, wrong disposition, and unsupported version. Assert caller refuses success and does not settle the unit or mutate another claim. Include the exact ack as the success control. |
| `lost_disposition_ack_replays_only_the_same_release` | Peer applies disposition, then drops the response. Repeat the exact disposition request through the transport seam. Assert identical ack and one release; no reserve, setup, pane allocation, or dispatch calls. Do not exercise a new placement request. |
| `disposition_and_dispatch_obey_both_gate_orders` | Use FIFO barriers around host dispatch and the disposition transaction. Release first: delayed new dispatch refuses. Dispatch first: matching published ownership survives; a transfer permits only that existing owner. Assert no false no-launch outcome. |
| `legacy_continuation_checks_fence_before_done_and_launch_locks` | Start from the existing legacy continuation fixture. With a prohibition fence, assert no input or mark mutation. With a matching transfer, continuation succeeds once. Hold the preparation gate and use bounded lock checks to prove `done` and launch locks were not acquired first. |
| `unknown_release_does_not_settle_possibly_launched_unit` | Start from a saved launch plus ambiguous host response. Lose or mismatch disposition acknowledgement. Assert the saved execution remains unresolved/possibly executed, with no relaunch and no `not_started` projection. |

**Mandatory unchanged negative control:**

```text
launch_retry::interrupted_job_admission_preserves_running_record_without_relaunch
```

Its existing assertions preserve the running Job, fail the ticket, and forbid a new launch receipt or dispatch (`tests/cli/launch_retry.rs:1634–1656`).

### C3 final acceptance

Run against the combined C3a+C3b revision:

```bash
cargo test --bin dogsled prepare::
cargo test --test cli launch_retry::
cargo test --test cli jobs::
cargo test --test cli legacy_continue::
cargo test --test cli unit_requests::
mise run check:pi
```

The lead owns the final combined check and records its exact SHA. Writers may run their focused red/green tests and formatting checks. Do not run extra full repository gates outside required commit hooks.

## C3 decisions and integration risks

| Decision/risk | Recommended default |
|---|---|
| Successful launch disposition could fence out its pending wrapper | Distinguish **transfer to exact existing owner** from **prohibit future startup**. Never use one unconditional “released means denied” flag. |
| Old peers understand path-only release but ignore fences | Require the new protocol at caller mutation boundaries; no downgrade fallback. Do not deploy C3a alone as the completed protocol. |
| Historical receipts lack identity | Keep them readable and conservative. Do not capture today’s checkout to repair historical authority. Requestless retirement remains out of scope. |
| Correlated release versus task outcome | Acknowledgement proves ownership release only. Unknown stays unknown; native no-launch evidence remains separately validated. |
| Shared files and unit size | Serialize C2 → C3a → C3b. Shared ownership is unavoidable in `work.rs` and `jobs.rs`; C3a/C3b also share `prepare.rs`, `main.rs`, and `launch_retry.rs`. Do not dispatch these concurrently. |

**Next action:** freeze the integrated main+C1 SHA, resolve the listed defaults, and dispatch C2 with that exact base.