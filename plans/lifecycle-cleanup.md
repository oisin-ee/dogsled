# Dogsled lifecycle: clear agent states and no leftover resources

Status: implementation approved. The follow-up interview approved the lifecycle and replay corrections below. Todo is the only execution tracker. Deployment and changes to existing host state need separate approval.

Research baseline: Dogsled `4dbbe3de5950d29837f4bfd146c7eb56fc23a0b1`. Both remote hosts run the 0.23.2 release (`8c65a19`), verified byte-for-byte against the published Linux executable.

## Context

### Problem 1: agent states are hard to read

The board mixes three vocabularies:

1. **Herdr terminal states:** `working`, `idle`, `blocked`, `done`, `unknown`. Herdr's `done` means "finished a turn that nobody has looked at yet". It does not mean the task is complete. Herdr's `blocked` means an approval or question dialog is on screen.
2. **Dogsled facts:** the child called `done`, `stalled`, `starting`, `unavailable`.
3. **Board groups** (`src/board.rs:417–426`): TO LAND, ATTENTION, RUNNING, WAITING. TO LAND means "finished, with commits not on main or with uncommitted changes". WAITING means "queued for host room". Check-ins also call an idle child "waiting" (`src/watchdog.rs:249`).

When a child ends its turn, Dogsled sees only `idle`. A child that asked a question looks the same as one that waits for its children, one that forgot `done`, and one that is stuck. The `done` tool text says an idle agent without `done` stays open (`src/mcp.rs:343`). Agents and the user cannot tell these cases apart.

### Problem 2: resources are left behind

A cleanup of netcup and momokaya-2 removed 44 stale panes, 23 worktrees, and 279 expired jobs and logs. Five dirty checkouts were saved as named Git stashes. Each kind of cruft has a cause in the source:

| Cruft | Cause |
|---|---|
| Stale panes | Children ended without `done`. Some hit the old todo gate. The guidance at `src/done.rs:13–19` is unclear. The sweep keeps an idle orphan forever (`src/sweep.rs:283`). The initial investigation found two retained examples: netcup `w15T:p1` and local `wRM:p1`. |
| Dirty checkouts | Owner close refuses uncommitted work (`src/close.rs:270–279`). The notice at `src/gate.rs:110` tells the owner to ask the user. |
| Detached checkout | `worktree::linked` rejects a detached HEAD (`src/worktree.rs:73–113`). Both close and `done` depend on it. |
| 11 preparation-held checkouts | Mostly legacy records. Diagnostic repairs `97b6f42`, `83790a6`, and `4dbbe3d` are not deployed. A lost placement reply never releases its hold (`src/work.rs:567–595`). Cancel does not release a hold (`src/work.rs:202–214`). |
| Expired jobs and logs | Job expiry runs only on manual prune. Prune also deletes the Job JSON with the result (`src/cleanup.rs:1892–1910`). |
| Ledger growth | No store prunes launches, admissions, queue tickets, items, `done` marks, or placement attempts. Local has 4,896 launch files (221 MB), 4,826 admissions, and 1,957 tickets in a 6 MB `queue.json`, which is rewritten on every ticket change. Netcup has 228 MB of launches. Every host sample reads every Job record into `host.json` (`src/node.rs:117`). |

### User decisions (2026-10-26)

| Topic | Decision |
|---|---|
| States | One lifecycle state per agent, each with a positive signal (table below). |
| "To land" | Replace it with the result `ready to merge` on a done item. It is not an agent state. |
| Questions | Agents call an explicit native `ask` tool. The question goes to the owner agent first. The owner escalates to the user only when it cannot answer. |
| Blocked and asking | Separate states. Blocked needs something fixed. Asking needs an answer. |
| Stopped agent | Dogsled prompts it once. If it stays stopped, it shows under the owner's attention. If the owner is gone, the orphan rule applies. |
| The word "done" | Means a recorded final handoff after native `done`. Before that, the agent is `finishing`. Herdr's `done` alone means the turn ended. |
| Unreadable agent | Show `unknown`, with the evidence gap. Never infer stopped or completed. |
| Closure and results | Keep agent state, resource closure, and result separate. READY TO MERGE contains retained branches, not recovery snapshots. |
| Orphan rule | Owner gone and child stopped: record its last reply, save its work, close it. |
| Saving work | On owner close and orphan close. Save all tracked and untracked content that Git stores without loss. Ignored files follow Git's unforced removal. Content that Git cannot store keeps the checkout. `done` still refuses uncommitted work. |
| Job retention | Delete logs after 24 hours. Delete conclusively terminal Job records after 30 days, even if their owner is offline. |
| Stale result references | An unread result or stale owner item does not extend terminal-job retention. The returning owner sees an expired result. |
| Execution validity | Require a valid recorded request. Refuse recordless or unprovably valid execution. Never manufacture a creation time. |
| Other records | The same 30-day rule applies to every store. Unresolved records never expire. |
| Hardening | Keep all of it. Check each unit's design against the source before dispatch. |
| No routine user review | The user never classifies files or approves each checkout. |

## Approach

### 1. Agent lifecycle

Each agent has exactly one state. Dogsled derives it in one function from positive evidence. Every consumer uses that function: board, sidebar, check-ins, sweep, and tool text.

| State | Meaning | Signal |
|---|---|---|
| `queued` | Waiting for admission. | Pending queue ticket without execution evidence |
| `starting` | Admission or startup is in progress, including preparation before pane creation. | Correlated admission or launch progress |
| `working` | In a turn. | Herdr `working` |
| `waiting` | Turn ended. Waiting for work it owns. | Open items or pending/admitting child tickets, with no explicit question or blocker |
| `asking` | Turn ended with a question for its owner. | Native `ask` call in that turn |
| `blocked` | Cannot continue until something outside it changes. | Native `blocked` call in that turn, or Herdr `blocked` (approval dialog) |
| `finishing` | Native `done` is accepted, but the terminal handoff is not complete. | The final reply, turn seal, or required unit acknowledgement is pending |
| `done` | The final handoff is recorded. Resource closure can still have a separate blocker. | Native `done`, sealed final reply, and required terminal evidence |
| `stopped` | Turn ended with no signal. | Idle, with no `done`, `ask`, `blocked`, or open children |
| `lost` | The assigned host is unavailable, or fresh evidence proves the launched pane is gone without a final handoff. | Host freshness and identity-correlated pane evidence |
| `unknown` | Dogsled cannot establish the current lifecycle state. | Unreadable, missing, or contradictory evidence |

A `working` agent with no transcript change past the stall limit shows "no output for N minutes" under NEEDS YOU. Silence never proves completion. Unknown and lost evidence never authorizes orphan closure.

Keep raw terminal observations for capacity and safety guards. Normalize Herdr `done` only inside lifecycle derivation. Missing agent data alone never means `starting`.

An explicit question or blocker outranks inferred waiting. Identical same-turn signals replay successfully; conflicting signals refuse without mutation. `asking` and `blocked` clear at the next positively identified turn. Pending delivery survives that change. A recorded final handoff stays terminal even when its host later becomes unavailable.

Board groups become what the owner must do:

| Group | Contains |
|---|---|
| NEEDS YOU | `asking`, `blocked`, `stopped`, `lost`, `unknown`, and stale-output or closure blockers |
| READY TO MERGE | An actual retained branch awaits integration |
| WORKING | `starting`, `working`, `waiting`, `finishing` |
| QUEUED | `queued` |

The result/history view also shows `merged`, `saved (recovery <id>)`, and `no changes`. A saved recovery does not make an abandoned task `done`. Resource closure stays independent of task outcome. Branch disappearance is not proof of merging.

Jobs keep their own small set: `queued`, `running`, `done` (exit 0), `failed (exit N)`.

### 2. Turn signals: `done`, `ask`, `blocked`

Add `ask` and `blocked` as native tools, CLI commands, and MCP tools beside `done`. Extend the existing mark store with typed nonterminal signals. Only `done` uses terminal sealing, inbox closure, and unit acknowledgement rules.

- `ask <question>` records a durable event bound to execution, harness session, and turn. The owner host imports it and notifies the owner with a stable event ID.
- `blocked <reason>` records a separate nonterminal event through the same route. Neither signal requires clean source or finished children.
- The owner answers with the existing `send`. The next positively identified turn clears the visible signal, not an undelivered event.

Questions and blockers have a typed item projection. They never replace the immutable final reply through `items::set_reply`. Correlate the recipient's session as well as host and pane, so replacement agents do not receive old questions.

Turn identity must survive restoration and missed observations. `completion_seq` alone is insufficient because restored sessions can reset it. Preserve legacy done decoding and reject ambiguous identity without changing terminal evidence.

`waiting` uses the same child-host ownership scan as the done guard: open items and pending/admitting tickets. Publish that evidence beside `done::observe`.

Rewrite the guidance for all three tools together, in `src/done.rs`, `src/mcp.rs`, and the Pi contract. The guidance says to end every turn with exactly one of: `done`, `ask`, `blocked`, or waiting for children. It must not present shell `dogsled done` as equal to the native tool (`src/done.rs:13–19`). Do not change or bypass pi-todo.

### 3. Stopped agents

When an agent becomes `stopped`, the watchdog sends it one prompt through its existing delivery path: "End your turn with `done`, `ask`, or `blocked`, or wait for your children." The prompt is recorded so it is sent once per stop.

- If the agent then signals, its state follows the signal.
- If it stays stopped and its owner is live, the row stays under NEEDS YOU with the reason.
- If its owner pane is gone, the sweep applies the orphan rule. It records the last reply on the item, saves the work (section 4), and closes the agent as an owner close would. A unit-bound orphan gets a resource-only close; its task binding stays unresolved.

The orphan rule closes only a positively identified `stopped` child. Working, waiting, asking, blocked, finishing, unknown, and lost states remain protected. Ordinary genuine done handoff keeps its existing authority and safety guards.

### 4. Saving work when an agent closes without `done`

Owner close and orphan close save uncommitted work, then remove the checkout. This replaces the refusal in `close::unsaved` and the "ask the user" notice in `src/gate.rs:110`. `done` keeps its refusal of dirty work.

- **Eligible:** a managed agent checkout (agent lock reason), close authority, no live activity, and content that Git stores without loss.
- **Snapshot:** use an isolated index (`GIT_INDEX_FILE`, `write-tree`, `commit-tree`). Record HEAD, index, working changes, untracked files, deletions, renames, and modes. Pin with `git update-ref` compare-and-swap under `refs/dogsled/recovery/<id>`.
- **Order:** write intent, build the snapshot, pin the ref, verify it, recheck that the live checkout is unchanged, then run teardown and unforced removal. If teardown changes source, snapshot again before removal. A failed or unknown teardown keeps the checkout.
- **Refusals:** conflicted index, dirty submodule, nested repository, unreadable path, or a file type Git cannot store. The checkout stays, and the item shows the reason.
- **Refs:** local, never pushed, never expired.
- **Restore:** native `recovery list`, `show`, and `restore`. Restore always uses a new checkout.
- **Report:** the recovery ID goes on the item and appears as the result `saved (recovery <id>)`.

### 5. Detached checkouts

Represent a linked checkout separately from its optional branch. Keep the existing empty-branch convention for detached worktrees in cleanup records. A detached HEAD is safe when main, refreshed remote refs, or a recovery ref holds it. Never create a branch implicitly. A `done` mark can have no branch.

### 6. Resource ownership hardening

These units close races that can leave holds behind or remove live work. Before dispatch, an architect checks each unit against the source at its base revision. The check confirms the smallest change and writes the failing test first. A contract change returns to the user.

**Lock order.** Take locks in this order, skip unused levels, and never take an outer lock from inside an inner one:

```text
origin admission/<id>
→ host preparation-receipts/<id>
→ Git common-directory lifecycle lock
→ done
→ launches/<id>
→ launch-panes/<pane>
→ short store locks
```

No queue lock is held across admission locks or RPCs. `prepare::used` stays a short leaf operation. Keep the `done → launch` order in continuation (`src/continuation.rs:65–67`). Take the repository lock before the final ownership check, and hold it through record reconciliation. Bind operations to the checkout's identity (Git administrative directory), not to its path.

**Same-checkout removal.** Close, prune, preparation, handoff, and preservation take the repository lock. If a checkout is already gone, the operation reconciles its records and does not report a failure. A replaced checkout at the same path is never removed. Reuse `Tree::drift`, `cleanup::gone`, `unless_gone`, and `validate_tree`.

**Job startup.** Under the gates, `work::launch` writes a pending ownership claim into the existing Job record. The `jobs::exec` wrapper rechecks the fence and checkout identity before it starts the process. Cleanup treats every nonterminal job as a hold. A failed ticket alone never authorizes removal or replay.

**Preparation disposition.** Add a versioned disposition receipt beside the existing preparation outcome. The host validates the request, writes the fence, releases only the matching claim, and returns a correlated acknowledgement. A mismatched record never releases.

**Placement replay.** Move `Action::Place` inside the admission lock. Extend `Attempt` through `recorded → host selected → prepared → result saved → release acknowledged → completed`. Save the result before release. `place --resume <request-id>` replays the same request with no new capacity, setup, or pane.

**Cancellation.** Record cancel intent under the admission lock. Dispatch rechecks it. An unreachable host leaves release pending. A saved launch stays "possibly executed". Only durable no-launch proof produces `not_started`.

**Retirement.** `retire-preparation <request-id>` ends future use of one historical claim. It records `outcome: unknown`, `future dispatch: prohibited`, and `ownership: released`. It never deletes a checkout and never settles the task. When the owner is gone, a separately authorized cleanup workflow supplies the authority and records who ran it and why. Requestless legacy holds are not supported.

### 7. Retention

The service timer runs retention. It does not need a Stop event or a doctor pass.

- **Job logs and stop markers:** delete 24 hours after `ended_ms`. First record `output_expired` in the Job record. Log reads report "output expired".
- **Job records:** delete strictly more than 30 days after authoritative terminal evidence. A failed transport response alone is not terminal proof. Explicit prune uses the same windows. A stale owner item or unread result does not extend this terminal-job window.
- **Other stores:** each owner deletes its own finished records older than 30 days, under its existing lock. This covers items, terminal tickets, launch receipts and `.sessions` transcripts, admissions, placement attempts, closed `done` marks, settled preparation receipts, dispositions, and fences. Reuse the inbox pattern (`src/inbox.rs:26`).
- **Never expire:** unresolved task bindings, held preparations, running/pending/uncertain jobs, or evidence still required by live execution. A historical pointer to a conclusively finished job is not live ownership.
- **Old requests:** record creation time once and propagate it unchanged. Refuse recordless or unprovably valid execution. Requests older than 30 days cannot start new execution, even if their unresolved ownership records must remain. Never derive creation time from request IDs or refresh it during retry.

Before metadata deletion, J1 requires an existing matching pending job at wrapper startup. J2 validates immutable request age at origin and host boundaries. Missing or corrupt execution evidence fails closed. A host replay cannot overwrite an existing claim. Ambiguous transport failure must preserve a concurrently started job or an uncertain pending claim.

Deletion and execution share the appropriate job locks. Validate timestamp ordering and prevent clock rollback from reopening expired requests. Retention must not delete lock files that another process can hold. Legacy records remain readable, but missing validity evidence never becomes permission to execute.

**J2 decisions (2026-10-26; the form timed out with the recommended options selected, so they hold until the user changes them):**

- **Receipt replay after expiry:** a correlated receipt replay stays read-only and allowed. It returns the saved result and never dispatches. Every path that can start new execution refuses an expired request.
- **Clock bootstrap:** each host keeps a persistent high-water clock. If the file is missing, create it automatically. Start it at the later of the current clock and the newest timestamp in the host's own records. A corrupt or unwritable clock refuses execution.
- **Future skew:** allow a creation time up to 5 minutes ahead of the host wall clock. Refuse anything further ahead. Never clamp or rewrite the creation time.

For remote jobs, authoritative execution-host terminal evidence starts retention. No owner read acknowledgement is required. After expiry, owner reads report an expired result rather than reviving the job.

Running jobs never expire. Age never ends ownership; it only removes records that are already finished.

### Out of scope

- Ticket-wide inspection, worker-result binding, and provider changes.
- Retirement of requestless legacy holds.
- Job retry changes, admission thresholds, and branch/base policy.
- Killing daemons or other external processes.
- Converting the five existing stashes into recovery refs.

## Files to modify

| Area | Paths |
|---|---|
| Lifecycle state and board | A single lifecycle owner; evidence in `src/probe.rs`, `src/node.rs`, `src/hosts.rs`, `src/items.rs`, `src/done.rs`, and launch progress. Consumers: `src/board.rs`, `src/watchdog.rs`, `src/sidebar/`, `src/gate.rs`, `src/commands.rs`, `src/context.rs`, `pi/board.ts`, `pi/work-panel.ts`. |
| Turn signals | `src/done.rs`, `src/mcp.rs`, `src/cli.rs`, `src/main.rs`, `src/runtime.rs`, `src/node.rs`, `src/hosts.rs`, `src/items.rs`, `src/inbox.rs`, and owned turn-boundary hooks in `pi/index.ts`. |
| Stopped agents and orphans | `src/watchdog.rs`, `src/sweep.rs` |
| Detached close | `src/worktree.rs`, `src/close.rs`, `src/done.rs` |
| Ownership hardening | `src/worktree.rs`, `src/prepare.rs`, `src/launch.rs`, `src/launch/recovery.rs`, `src/work.rs`, `src/commands.rs`, `src/close.rs`, `src/cleanup.rs`, `src/jobs.rs`, `src/continuation.rs`, `src/unit_request.rs`, `src/cli.rs`, `src/main.rs`, `src/runtime.rs`, `src/mcp.rs` |
| Saving work | New `src/preservation.rs`; `src/close.rs`, `src/sweep.rs`, `src/loss.rs`, `src/items.rs`, `src/gate.rs`, `src/cli.rs`, `src/main.rs`, `src/runtime.rs`, `src/mcp.rs` |
| Retention and replay prerequisites | `src/jobs.rs`, `src/work.rs`, `src/runtime.rs`, `src/cli.rs`, `src/main.rs`, `src/cleanup.rs`, `src/service.rs`, `src/watchdog.rs`, `src/node.rs`, and each store owner listed in section 7 |
| Tests | `src/board.rs` tests, `src/sidebar/view/tests.rs`, `src/watchdog_tests.rs`, `src/loss_tests.rs`, `src/launch/recovery_tests.rs`, `pi/board.test.ts`, `pi/unit-contract.test.ts`, `tests/cli/*.rs`, new `tests/cli/signals.rs`, `tests/cli/preservation.rs`, `tests/cli/preparation_retirement.rs`, `tests/cli/retention.rs`, registered in `tests/cli.rs` |
| Documentation | `README.md`, `docs/design.md`, `docs/pi-work.md`, `docs/unit-requests.md`, `CHANGELOG.md` |

## Reuse

| Existing mechanism | Use |
|---|---|
| `done::Mark` store and `done::observe` (`src/done.rs`) | Hold `ask` and `blocked` signals; publish signals in the host snapshot |
| `items::set_reply`, `items::merge_handoffs`, `inbox::notify` | Preserve final replies; import typed resource and signal metadata through existing stores; notify on the owner's host without rewriting terminal results |
| `close::local` and its guards | Owner close and orphan close, with preservation in place of the dirty refusal |
| `sweep::children` and `settle` (`src/sweep.rs:206–330`) | The orphan rule |
| `watchdog::observe` stall tracking | The "no output" evidence gap and the one-time stopped prompt |
| `loss::Evidence::assess` | Removal safety after preservation |
| `Tree::drift`, `cleanup::gone`, `unless_gone`, `validate_tree` | Same-checkout races |
| `prepare::pending_trees`, `prepare::used`, `launch::unit_receipt` | Preparation release |
| `Ledger::with_lock`, `Ledger::write` (`src/ledger.rs:76–153`) | All locks and atomic writes |
| `Shell::run_input_bounded` (`src/shell.rs:43–52`) | Git plumbing input with deadlines |
| `inbox` `KEEP_MS` pruning (`src/inbox.rs:26`) | Retention pattern for other stores |

## Steps

Each unit is one worker assignment. A unit starts only when its prerequisites are integrated. Units that share a file never run at the same time.

- **L1 — Lifecycle evidence and consumers.** Derive the approved states, including finishing and unknown, without overwriting raw terminal evidence. Keep closure and result evidence separate. Split the evidence/classifier, Rust consumers, and Pi consumers into bounded units. Coordinate the binary and Pi package rollout; existing installed Pi code cannot gain fallback behavior retroactively. Prerequisite: C7 before shared done/result edits. Acceptance: lifecycle classifier tests, `cargo test --bin dogsled board::`, `cargo test --bin dogsled sidebar::`, `mise run check:pi`.
- **C7 — Detached close and done.** Section 5. Prerequisite: none. Acceptance: `cargo test --test cli close::` and `cargo test --test cli done::`. A clean held detached checkout closes natively. Dirty or unheld detached HEAD stays.
- **J1 — Fail-closed job execution.** Require a matching pending claim inside the execution lock. Refuse missing/corrupt/terminal/uncertain claims. Never overwrite an existing claim on host replay. Preserve uncertainty after ambiguous dispatch failure. Prerequisite: accepted retention design. Acceptance: `cargo test --test cli jobs::`, `cargo test --test cli lifecycle::`, `cargo test --test cli launch_retry::`.
- **J2 — Recorded request validity.** Record creation time once. Validate it at origin and host before job side effects, with explicit legacy and clock behavior. Prerequisite: J1. Acceptance: job and launch-replay tests prove that stale or unprovable requests cannot restart after metadata deletion.
- **C8 — Job log and record retention.** Section 7, jobs only. Prerequisite: J1 and J2. Acceptance: `cargo test --test cli jobs::` and `cargo test --test cli stop_sweep::`. Logs go at 24 hours and records at 30 days. A running job never expires. Expiry repeats safely. No Stop event or doctor pass is needed.
- **L2 — Turn signals.** Section 2, including the rewritten guidance. Prerequisites: L1, C7, C8. Acceptance: `cargo test --test cli signals::`, `cargo test --test cli done::`, `cargo test --test cli mcp::`, `mise run check:pi`. `ask` reaches the owner's inbox and item. `blocked` shows its reason. The next turn clears both. An agent with open children shows `waiting`.
- **L3 — Stopped prompt and orphan close.** Section 3, for clean checkouts. Prerequisite: L2. Acceptance: `cargo test --test cli watchdog::` and `cargo test --test cli sweep::`. A stopped agent gets one prompt. A live owner sees it under NEEDS YOU. A clean orphan gets its reply recorded and closes. A working, waiting, asking, or blocked agent stays.
- **C1 — Repository lock and verified absence.** Section 6, lock order and same-checkout removal. Prerequisites: C7, C8, L2. Acceptance: `cargo test --test cli launch_retry::` and `cargo test --bin dogsled loss_tests::`. Concurrent close and prune do one teardown. A prior removal reconciles without failure. A replacement checkout stays.
- **C2 — Job ownership through startup.** Prerequisite: C1. Acceptance: `cargo test --test cli jobs::`. Both race orders, dispatch before removal and removal before dispatch, keep live work or refuse stale startup.
- **C3 — Preparation dispositions and fences.** Prerequisite: C2. Acceptance: `cargo test --bin dogsled prepare::`, plus `launch_retry::`, `jobs::`, `legacy_continue::`, and `unit_requests::` CLI tests, and `mise run check:pi`. Keep `interrupted_job_admission_preserves_running_record_without_relaunch` as a negative control.
- **C4 — Placement replay.** Prerequisite: C3. Acceptance: `cargo test --test cli place::`. A lost reply or acknowledgement resumes the same request with no new capacity, setup, or pane.
- **C5 — Cancellation under admission.** Prerequisite: C4. Acceptance: `cargo test --test cli unit_requests::`. A cancel and dispatch race keeps possibly executed work. Nothing becomes `not_started` without proof.
- **C6 — Request-scoped retirement.** Prerequisite: C5. Acceptance: `cargo test --test cli preparation_retirement::`. Two claims on one path stay independent. Output says the resource is retired and the task is unresolved. Requestless claims refuse.
- **P1 — Recovery snapshots.** Section 4, snapshot and verify only. Prerequisite: C1. Acceptance: `cargo test --bin dogsled preservation::`. Every recorded content kind round-trips. Unsupported content refuses with a reason. A failed write never clears live content.
- **P2 — Recovery list, show, restore.** Prerequisites: P1, C6. Acceptance: `cargo test --test cli preservation::`. Restore uses a new checkout. Refs survive restart and are never pushed.
- **R1 — 30-day retention for other stores.** Section 7. Prerequisites: C6, C8. Acceptance: `cargo test --test cli retention::`. Only finished records older than 30 days go, each deleted by its own store. Open, held, unsettled, and referenced records stay. Requests older than the window are refused.
- **P3 — Save on owner close and orphan close.** Prerequisites: P2, L3, R1. Acceptance: `cargo test --test cli preservation::` and `cargo test --test cli sweep::`. A dirty abandoned child is saved, removed, and reported with its recovery ID. A dirty orphan gets the same treatment. `src/gate.rs` no longer tells the owner to ask the user.
- **D1 — Actionable holds.** Every kept resource names its owner and the action that releases it. Prerequisite: P3. Acceptance: `cargo test --test cli stop_sweep::`. Retirement never appears as task settlement.
- **D2 — Documentation.** The lifecycle states, signals, recovery commands, retention, and migration. Prerequisite: all units above. Acceptance: review against the implemented CLI and wire behavior.
- **Integration.** `mise run check:rust` and `mise run check:pi` at the final revision.

Order summary: C7 and J1 can run in parallel. L1 evidence follows C7; J2 follows J1. C8 follows J2. L2 follows the lifecycle consumers and retention prerequisites. Serialize shared-path owners in todo. Later hardening, preservation, and per-store retention follow their integrated prerequisites. R1 must extend request-validity enforcement to preparation, placement, and agent dispatch before deleting their replay evidence.

## Verification

Each unit first adds a failing regression test at its owning seam. Its focused tests must pass at the unit's final revision. The integrated revision runs `mise run check:rust` (format, Clippy, tests) and `mise run check:pi` (TypeScript and Node tests) once. Evidence for each unit names the command, the revision, the scenario, the result, and any coverage gap.

Required scenarios beyond each unit's acceptance:

| Area | Scenario | Expected |
|---|---|---|
| States | One agent passes through queued, starting, working, asking, working, waiting, and done. | Each step shows exactly one state. The board group matches. |
| States | Herdr reports `done` without a native completion, a handoff is pending, or terminal evidence is unreadable. | Derive stopped/asking/waiting from evidence; pending handoff is finishing; missing evidence is unknown. Never invent done. |
| Signals | `ask` and `done` in the same turn, duplicate delivery, or restored counters. | Conflicting signals refuse without mutation. Identical retries deduplicate. Distinct restored turns retain distinct events. |
| Stopped | Agent stops twice in separate turns. | One prompt per stop. Never a prompt loop. |
| Orphans | Owner pane gone; child working, asking, blocked, waiting, finishing, unknown, or lost. | The orphan rule keeps the child and its evidence. |
| Results | A checkout is saved after abandonment; a retained branch disappears. | Saved content is not task completion. Branch absence is not proof of merging. |
| Saving | Staged and unstaged changes in one file, binary files, deletions, renames, executable modes, untracked files. | Restore reproduces the index and working tree exactly. |
| Saving | Crash before pin, after pin, before removal; content changes during the snapshot. | A recoverable copy always exists. Resume or refuse. Never clear before the ref is pinned. |
| Saving | Teardown fails, changes source, or crashes. | The checkout stays, or post-hook changes are saved before removal. |
| Races | Close and prune on the same checkout; HEAD moves; path is replaced. | One removal. A moved HEAD is skipped. The replacement stays. |
| Preparation | Crash between host preparation and caller save; two requests on one path; peer unreachable. | Same-request recovery. Only the matching claim releases. Unknown stays unknown. |
| Retention | Exact 24h/30d boundaries; restart without Stop; wrapper and host replay after Job and receipt deletion. | Strict boundaries. No duplicate command runs. Missing claims and expired requests refuse. |
| Retention | Owner offline beyond 30d; transport failure races wrapper startup; corrupt records or clock rollback. | Conclusive terminal jobs expire despite stale result pointers. Uncertain execution stays protected. Clock changes cannot reopen expired requests. |
| Processes | An external daemon holds a checkout, with secrets in its arguments. | Show owner and process identity without arguments. Never kill it. |

No build or test ran during planning.

## Rollout and existing state

1. Release the integrated revision. It includes the undeployed diagnostic repairs `97b6f42`, `83790a6`, and `4dbbe3d`.
2. Deploy to peer hosts before callers use new protocol actions. An older peer must return a clear "unsupported" result. Once retirement fences exist, never downgrade below a version that understands them.
3. Canary on one host. Verify the installed artifact and service health. Run a preview that lists what the first retention pass and the first orphan sweep would delete or close. The user approves the preview before the first real pass.
4. With separate authorization, retire the 11 preparation-held netcup checkouts through `retire-preparation`. Then normal close and prune apply. `feat-cup-pages` (requestless) stays and is reported.
5. The five protected panes stay until their own evidence changes: netcup `wTM:p1`, `w15T:p1`, and `w18N:p1`, and momokaya-2 `w1D:p2` and `w2F:p1`. After L3 and P3, `w15T:p1` (orphan, stopped) becomes eligible for the orphan rule.
6. The five archived stashes stay as named stashes in their host repositories.

Rollback stops new retirement and retention actions. It never erases receipts, recovery refs, or fences.
