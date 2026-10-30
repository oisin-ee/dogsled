# Agent lifecycle and recovery

This guide describes the behavior in the current `main` source.
It does not describe the versions installed on fleet hosts.
The [approved plan](../plans/lifecycle-cleanup.md) also includes work that is not yet released.

## Agent states

Dogsled derives one lifecycle state from recorded evidence.
Herdr terminal observations remain separate inputs for capacity and safety decisions.
Herdr `done` means that a turn ended. It does not prove task completion.

| State       | Evidence and meaning                                                                                               |
| ----------- | ------------------------------------------------------------------------------------------------------------------ |
| `queued`    | A pending ticket waits for admission, without execution evidence.                                                  |
| `starting`  | Correlated admission or launch evidence shows startup in progress.                                                 |
| `working`   | Herdr reports an active turn.                                                                                      |
| `waiting`   | The turn ended. The agent owns open items or pending or admitting child tickets.                                   |
| `asking`    | The turn ended with an explicit `ask` signal. The agent needs an answer.                                           |
| `blocked`   | The agent recorded a `blocked` signal, or Herdr reports an approval dialog.                                        |
| `finishing` | Dogsled accepted `done`. The final reply, turn seal, or required terminal evidence remains pending.                |
| `done`      | Dogsled recorded the final handoff and its required terminal evidence.                                             |
| `stopped`   | The turn ended without a signal or open child work.                                                                |
| `lost`      | The assigned host is unavailable, or correlated evidence proves that the pane disappeared without a final handoff. |
| `unknown`   | Missing, unreadable, or contradictory evidence prevents a state decision.                                          |

A recorded handoff remains terminal if its host later becomes unavailable.
Silence never proves completion. Missing agent data alone never proves startup.

The board separates agent state, resource closure, and retained results:

| Group          | Meaning                                                                                                |
| -------------- | ------------------------------------------------------------------------------------------------------ |
| NEEDS YOU      | Questions, blockers, stopped agents, lost agents, unknown evidence, stale output, or closure blockers. |
| READY TO MERGE | A retained branch awaits integration. This group describes a result, not an agent state.               |
| WORKING        | Agents in `starting`, `working`, `waiting`, or `finishing`.                                            |
| QUEUED         | Tickets in `queued`.                                                                                   |

A working agent with stale transcript output can require attention without a state change.
A missing branch does not prove a merge.

Source: [`src/lifecycle.rs`](../src/lifecycle.rs) and [`src/board.rs`](../src/board.rs).

## End a turn

End each turn with one of four outcomes:

- Call the native `done` tool for the final handoff.
- Call the native `ask` tool for a question to the owner.
- Call the native `blocked` tool for an external blocker.
- Wait for child work that the agent owns.

There is no `waiting` command. Dogsled derives that state from ownership records.
The CLI also exposes these signals:

```sh
dogsled done
dogsled ask "Which API owns this operation?"
dogsled blocked "The required host is unavailable."
dogsled send CHILD --message "Use the repository API."
```

For managed agents, use the native tool, not shell `dogsled done`, for final completion.
The native tool preserves the harness handoff boundary.
Call it during the final turn, then end that turn with the final reply.

`done` refuses open children, pending or admitting child tickets, and uncommitted work in a managed agent checkout.
A local retained branch protects commits during normal handoff.
The child host captures the final reply and closes eligible resources after the turn ends.
Closure can still refuse when a safety condition changes.

`ask` and `blocked` do not complete the task.
They do not require a clean checkout or finished children.
Each signal binds to the execution, harness session, and turn.
An identical same-turn retry succeeds. A conflicting same-turn signal refuses without a change.
An explicit question or blocker takes priority over inferred `waiting`.

The owner receives the question or blocker through its inbox.
The owner answers through `send`.
The next positively identified turn clears the visible signal, but not an undelivered event.
Questions and blockers never replace the final reply.

Source: [`src/done.rs`](../src/done.rs), [`src/items.rs`](../src/items.rs), and [`src/inbox.rs`](../src/inbox.rs).

## Stopped agents and orphans

The watchdog sends a recorded prompt for an eligible stop:

> End your turn with done, ask, or blocked, or wait for your children.

The prompt uses a stable message ID. Delivery retries do not create another prompt.
The watchdog limits prompts across the same session to prevent a prompt loop.
A stopped agent with a live owner remains under NEEDS YOU.

If the current custodian is gone, the sweep can close a positively identified stopped orphan.
It records the last reply before closure.
A unit-bound orphan receives resource-only closure. Its task binding remains unresolved.
This path does not create a final handoff or prove task completion.

Owner close and stopped-orphan close save uncommitted work in pinned recovery snapshots before checkout removal.
If preservation cannot prove that content is saved, the checkout remains.
The orphan rule protects `working`, `waiting`, `asking`, `blocked`, `finishing`, `unknown`, and `lost` agents.
Ordinary recorded `done` handoffs use their own closure rules.

Source: [`src/watchdog.rs`](../src/watchdog.rs), [`src/sweep.rs`](../src/sweep.rs), and [`src/close.rs`](../src/close.rs).

## Close and inspect recovery

```sh
dogsled close CHILD --host HOST
dogsled recovery list --repo /path/to/repo
dogsled recovery show RECOVERY_ID --repo /path/to/repo
dogsled recovery restore RECOVERY_ID --repo /path/to/repo --to /path/to/new-checkout
dogsled recovery restore RECOVERY_ID --repo /path/to/repo --to /path/to/new-checkout --branch recovered/work
```

`close` refuses busy work and removal that loses unsaved content.
Explicit owner close and stopped-orphan close pin verified recovery snapshots before they remove supported dirty checkouts.
These paths can retain unmerged commits locally without pushing them.
Normal handoff, prune, and other cleanup paths retain their existing loss checks.
Explicit close can restore tracked deletions alone when retained refs protect the tip.
Managed checkouts use the lock reason `agent:worktree`.
Shared checkouts remain while another agent, open item, or busy pane uses them.
A clean detached checkout can close when the default branch or freshly fetched remote refs protect its HEAD.
Dogsled does not create a branch implicitly for a detached checkout.
Close saves the initial snapshot before teardown.
After successful teardown, close saves changed content again before normalization and unforced removal.
A failed or unknown teardown keeps the checkout and its resources.
Dogsled does not automatically repeat an entered teardown with an unknown outcome.
An interrupted normalization also keeps the checkout for inspection.
Shared ownership, changed checkout identity, and unsupported content prevent removal.

Recovery snapshots keep HEAD, the staged index, and working content in local Git refs.
They can preserve untracked files, binary content, deletions, renames, symbolic links, and executable modes.
The snapshot code refuses content that Git cannot store without loss.
Examples include a conflicted index, a dirty submodule, a nested repository, and unsupported file types.
Snapshots exclude ignored files.
Dogsled neither pushes nor expires `refs/dogsled/recovery/<id>`.

`list` and `show` inspect archived snapshots after the original checkout disappears or the process restarts.
Invalid entries remain visible with an error.
There is no separate public snapshot-creation command.
Owner close and stopped-orphan close create snapshots through durable preservation operations.
A successful preservation reports `saved (recovery <id>)` and retains recovery metadata with the resource result.
This result does not complete the original task or prove that its branch merged.

`restore` always creates an independent checkout at a new destination.
It refuses an existing destination or a changed recovery ref.
It restores the saved index, exact file bytes, and modes without setup hooks or content filters.
Without `--branch`, it restores a detached HEAD when the snapshot has a commit.
The optional branch must be new.

The restored checkout does not adopt the original task, admission, preparation, capacity grant, pane, or custody.
Restore does not resume the original execution or complete its task.
The recovery ref remains pinned after restore.

The recovery command accepts an explicit `--host HOST` before its subcommand.
It does not select another host or fall back to local recovery after a peer error.

Source: [`src/preservation.rs`](../src/preservation.rs), [`src/runtime.rs`](../src/runtime.rs), and [`src/cli.rs`](../src/cli.rs).

## Repository lock and checkout identity

Repository lifecycle operations share a lock in the canonical Git common directory.
Close, prune, preparation, handoff, and recovery restore use this lock before their final ownership decisions.
They retain protection through record reconciliation.

A path alone does not identify a checkout.
The identity includes the Git common directory, administrative directory, and a write-once checkout generation.
A replacement checkout at the same path does not inherit removal authority.
Verified absence permits record reconciliation. An unreadable or changed identity does not prove absence.

The outer lock order is admission, preparation receipt, repository lifecycle, `done`, launch, then launch pane.
Short store locks come last. Callers skip unused levels.
Callers release queue locks before they acquire admission locks or make peer requests.

Source: [`src/worktree.rs`](../src/worktree.rs), [`src/close.rs`](../src/close.rs), and [`src/cleanup.rs`](../src/cleanup.rs).

## Request validity

Preparation, placement, agent dispatch, and job execution retain one immutable request creation time.
The origin records validity once and sends the same evidence through admission and startup.
The execution host refuses missing, changed, unsupported, expired, or excessive future-dated validity before new side effects.
It rechecks validity at preparation and dispatch boundaries.
A refusal does not release an unresolved preparation claim.
Request-scoped retirement remains a separate operation.

Each owner records `finished_ms` once when its own terminal evidence permits completion.
Startup confirmation is not a completion time.
Missing completion times and contradictory timestamps prevent retention proof.
These timestamps do not implement the pending R1, part 2 expiry rules.

Source: [`src/jobs.rs`](../src/jobs.rs), [`src/prepare.rs`](../src/prepare.rs), [`src/node.rs`](../src/node.rs), and [`src/launch.rs`](../src/launch.rs).

## Job startup ownership

The execution host records ownership before job startup.
The job wrapper requires the exact pending claim before it starts a process.
A missing, corrupt, changed, started, ended, or uncertain claim cannot authorize another execution.
A host replay does not overwrite an existing claim.

Before process startup, the wrapper rechecks request validity, the preparation fence, and checkout identity.
For non-repository jobs, it rechecks the recorded directory scope.
Cleanup treats every job without conclusive terminal evidence as a hold.
A failed ticket or transport error alone does not release that hold.

Job requests retain their original creation time.
Requests older than 30 days cannot start new execution.
A persistent high-water clock prevents clock rollback from reopening expired requests.
A missing clock bootstraps from the wall clock and local record timestamps.
A corrupt or unwritable clock refuses execution.
A creation time more than five minutes ahead of the wall clock also refuses execution.
Read-only receipt replay does not authorize new execution.

Source: [`src/jobs.rs`](../src/jobs.rs) and [`src/work.rs`](../src/work.rs).

## Preparation dispositions and placement replay

A disposition records what happens to one preparation claim.
The versioned receipt binds the request, checkout identity, claim, policy, and execution outcome.
The host validates that identity before it writes a startup fence.
It then releases only the matching claim and returns a correlated acknowledgement.
A conflicting receipt or claim remains held.

The fence can transfer ownership to a recorded launch or prohibit future startup.
Ownership release is not proof that execution never started.
A retained fence prevents an old request from starting after release.

Placement retains the selected host and its durable capacity grant.
Grant replay uses the original decision rather than another capacity admission after the reservation TTL.
The journal records preparation, the saved result, release acknowledgement, and completion.
Dogsled saves the result before release.

If a placement reply or acknowledgement is lost, resume the recorded request:

```sh
dogsled place --resume REQUEST_ID --json
```

Resume uses the original request, host, grant, and preparation envelope.
It does not allocate another grant, repeat completed setup, or create another pane.
Completed replay only reads the saved result.
`--resume` cannot combine with `--host`, `--branch`, `--base`, or `--repo`.

Source: [`src/prepare/disposition.rs`](../src/prepare/disposition.rs), [`src/work/placement.rs`](../src/work/placement.rs), and [`src/work.rs`](../src/work.rs).

## Cancellation and retirement

```sh
dogsled cancel TICKET
dogsled retire-preparation REQUEST_ID --host HOST --reason "Historical claim no longer has an owner."
```

Cancellation records intent under the admission lock. Dispatch rechecks that intent.
After admission, cancellation returns `status: "too_late"` with the saved result.
Cancellation never stops a process.
An unreachable host leaves release pending.
The service retries a pending release after admission, outside the admission lock, at most 16 per pass.
A cancellation with no saved preparation identity is held as `cancellation_held`. Its release stays pending.
The service retries a held release only when its admission, preparation, or retirement records change.
An acknowledged `retire-preparation` for the same request releases it with `release: "retired"`.
A saved launch remains possibly executed until durable evidence proves otherwise.
Only no-launch proof permits `not_started`.

`retire-preparation` prohibits future dispatch for one historical request claim.
After host acknowledgement, it reports `outcome: "unknown"`, `future_dispatch: "prohibited"`, and `ownership: "released"`.
It also reports `task: "unresolved"`.
Retirement never removes a checkout, kills a process, or settles a task.
Claims without request IDs are unsupported.

For a separately authorized operator cleanup, the CLI accepts `--cleanup` with the required `--reason`.
This override records the actor and authority for a historical claim without owner provenance.
MCP does not expose the cleanup override.
Retirement still releases only the matching claim. Other claims on the same checkout remain independent.

Source: [`src/work.rs`](../src/work.rs) and [`src/preparation_retirement.rs`](../src/preparation_retirement.rs).

## Child-event custody and origin release

Child replies, questions, and blockers follow the current custodian.
A claim does not change the original owner or task.
Delivery captures the child identity and custody revision, then rechecks both through the inbox write.
A stable event ID removes duplicates. It does not grant delivery authority.

A missing child item, changed custody, or replacement recipient session does not authorize pane-only delivery.
Dogsled retains a diagnostic for an undelivered or superseded notification.
Receipt delivery and task acceptance remain separate operations.

Origin release uses an explicit, versioned certificate after the provider releases the exact binding.
The certificate names the origin host, execution, binding, launch, terminal evidence, and custody revision.
It must release all four reference classes: execution, recovery, task binding, and custody.
These protocol recovery references are not the pinned Git recovery refs.

The origin saves its release intent before it contacts the execution host.
The host compares the certificate with its retained launch and terminal evidence under the launch lock.
An identical retry returns the saved acknowledgement. A changed certificate refuses.
A lost reply leaves the origin intent available for replay.
An inbox acknowledgement, settlement acknowledgement, silent owner, or offline origin does not prove reference release.

The protocol commands are:

```sh
dogsled unit release-references --input '<ReferenceRelease JSON>'
dogsled host release-references --input '<ReferenceRelease JSON>'
dogsled host reference-release-status --input '<ExecutionRef JSON>'
```

The `host` commands are internal peer operations, not placement commands.
Without a certificate, status reports `awaiting origin reference release` and a retry action.
The certificate does not itself remove records or complete a task.

Source: [`src/inbox.rs`](../src/inbox.rs), [`src/launch.rs`](../src/launch.rs), and [`src/unit_request.rs`](../src/unit_request.rs).

## Resource hold reports

`status`, the board, and MCP show retained resources separately from task states.
Each report names the resource, host, owner, reason, and next command.
JSON includes `code`, `command_host`, and `command_argv` as well as the quoted command text.
Remote commands name the host on which they must operate.
An unavailable host retains its last observed ownership with an explicit warning.

The shared table covers preparation claims, capacity, admissions, cancellation, checkout ownership, retained branches, and origin references.
It also covers request-validity refusals, missing or inconsistent completion times, preservation blockers, and undelivered child events.
A teardown blocker can point to `recovery show` for the pinned snapshot.
Inspection commands do not grant release or retry authority.
A plausible completion time alone does not prove terminal execution or reference release.

Source: [`src/holds.rs`](../src/holds.rs), [`src/commands.rs`](../src/commands.rs), [`src/board.rs`](../src/board.rs), and [`src/mcp.rs`](../src/mcp.rs).

## Retention

The service timer runs job retention without a Stop event or a doctor pass.
Only authoritative terminal evidence on the execution host starts the retention period.
A transport error alone does not prove termination.

| Data                               | Retention                                                                                                                           |
| ---------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------- |
| Job output and stop marker         | More than 24 hours after `ended_ms`. The Job record first gains `output_expired`. A small `output expired` marker replaces the log. |
| Terminal Job record                | More than 30 days after `ended_ms`. The pass removes the JSON record and the remaining log marker.                                  |
| Running, pending, or uncertain job | No age-based expiry without conclusive terminal evidence.                                                                           |
| Git recovery ref                   | No automatic expiry.                                                                                                                |

A stale owner item or unread result does not extend terminal-job retention.
The current reader reports `output expired` for expired output.
A retained job item can also produce this response after the Job record disappears.
Other lookups without retained evidence can report an unknown job.
Expiry does not recreate a deleted job or authorize execution.

Explicit `prune jobs` currently has a shorter window.
It can remove a conclusively terminal job record, output, and stop marker after more than 24 hours.
This manual path does not yet use the service timer's 30-day record window.

Source: [`src/jobs.rs`](../src/jobs.rs), [`src/runtime.rs`](../src/runtime.rs), and [`src/service.rs`](../src/service.rs).

## Migration

The current CLI can read legacy records for inspection.
Legacy records without execution-validity evidence do not authorize new job execution.
Old peers cannot safely accept unsupported preparation, placement, or job protocols.

The rollout plan requires compatible peers before callers use the new protocol actions.
After retirement fences exist, the plan prohibits downgrade to versions that ignore those fences.
Deployment and cleanup of existing host state require separate approval.
Rollback must retain receipts, fences, and recovery refs.

## In progress

These parts of the approved plan remain pending in the source:

- **R1, part 2:** Expire finished records in the other stores after 30 days, while unresolved or live references remain protected.

The plan also requires explicit prune to use the service retention windows. That alignment is not yet implemented.
R1, part 2 also includes the reader message `result unavailable: expired or unknown`.
That message is not the current reader behavior.
