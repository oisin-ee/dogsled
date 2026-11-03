<!-- USER DECISIONS 2026-10-29: (1) P2 starts now, before C6, with the independent-checkout boundary. (2) R1 D1: require a versioned origin-release certificate. (3) R1 D2: no permanent tombstones; after the last identity expires, the job is removed completely and reads return "result unavailable: expired or unknown". (4) R1 D3: drop a child event whose subject disappeared, with a visible reason. (5) P3 and P2 architect defaults accepted. -->
<!-- P2/P3/RV contracts at d7c7303 -->

# Dispatch basis

**Reviewed revision:** `d7c7303a91c92539feb95c393abfb524c066e248`.

These contracts describe proposed edits, not implemented behavior. No files changed. No builds or tests ran.

`origin/main` advanced during inspection. The reviewed `src/` files remained identical to the requested revision. I reread the changed build configuration and job tests from the fixed SHA.

**Dispatch gates:** P2 retains its approved C6 prerequisite unless the user approves the exception below. RV follows C6 and precedes R1. P3 follows P2, L3, and R1. The lead must bind each writer to its actual landed prerequisite SHA.

## Rules common to all three units

| Field | Contract |
|---|---|
| Repository | `dogsled` |
| Assignment | One writer per unit. Serialize shared-path ownership. |
| Expectation capture | Capture each complete expectation once. Compare subsequent observations against it. Never replace the expectation during validation. |
| Refusal | Preserve resources and evidence. Report the exact reason and the recovery, retry, or retirement action. |
| Fixtures | Use short `/tmp` roots and `git init -b main`. Use `worktree::same` for production path comparisons and corresponding normalized assertions in CLI fixtures. |
| Service fixtures | Before `serve --once --interval 1`, set zero admission thresholds through `rig.config` and seed `host.json`. |
| Test environment | Run direct `cargo` commands with the normal PATH, excluding mise shim directories. Do not use `mise x`. |
| Checks | Add the owning regression first. Record its failure before implementation and success afterward. Preserve negative controls. |
| Generated files | None. Do not edit `web/public`, dependencies, installed binaries, or local version pins. |
| Publication | Local implementation commits through normal hooks. No push, PR, merge, release, deployment, or existing-host cleanup authority. |
| Writer handoff | Branch, full SHA, changed paths, red/green commands and revisions, remaining risks, and coverage gaps. |

Service lanes run concurrently. Lane order is not synchronization: `src/service.rs:run:164–175`.

### Lock order

```text
origin admissions/<request-id>
→ host preparation-receipts/<request-id>
→ Git common-directory lifecycle lock
→ done
→ launches/<request-id>
→ launch-panes/<pane-digest>
→ short store locks
```

Skip unused levels. Never acquire an outer lock from an inner level.

The lifecycle mutex belongs to the canonical Git common directory: `src/worktree.rs:with_lifecycle:176–187`. Checkout identity includes the administrative directory and generation: `src/worktree.rs:Identity:26–39`.

Do not hold `queue.lock` across admission locks or RPCs. Keep `prepare::used` a short exact-match operation: `src/prepare.rs:used:1441–1458`. Preserve continuation’s `done → launch` ordering: `src/continuation.rs:continue:69–95`.

---

# P2 — Recovery list, show, and restore

**Cause:** P1 creates durable recovery refs, but it exposes no production reader or restore operation, and its verifier requires the original checkout.

## Evidence

| Source at d7c7303 | Finding |
|---|---|
| `src/preservation.rs:Snapshot:68–81` | The snapshot records HEAD, separate index and working trees, raw-byte semantics, and creation time. |
| `src/preservation.rs:Record:83–94` | The recovery commit message contains the durable metadata. |
| `src/preservation.rs:snapshot:661–701` | Capture creates the commits, pins the ref, verifies it, and leaves live content unchanged. |
| `src/preservation.rs:verify:705–759` | Verification also compares the live HEAD, index, and working tree. It cannot verify an archived snapshot after removal. |
| `src/preservation.rs:tests::Repo::round_trip:800–865` | Restore exists only as test code. |
| `src/shell.rs:Output:9–14`, `src/shell.rs:System:195–198` | Captured stdout is a `String`, produced through lossy UTF-8 conversion. It cannot transport arbitrary blob bytes for restore. |
| `src/cli.rs:Command:26`, `src/main.rs:41` | The binary includes preservation, but the public command enum has no recovery command. |

## Deliverable and scope

**Deliverable:** Native, host-scoped recovery inspection and byte-exact restoration into a new checkout.

**Suggested branch:** `feat/recovery-commands`.

| Scope | Paths |
|---|---|
| Owned production | `src/preservation.rs`, `src/shell.rs`, `src/cli.rs`, `src/main.rs`, `src/runtime.rs`, `src/mcp.rs` |
| Owned tests | `tests/cli/preservation.rs` (new), `tests/cli.rs`, `tests/cli/mcp.rs` |
| Read | `src/worktree.rs`, `src/prepare.rs`, `src/prepare/disposition.rs`, `src/work/placement.rs`, `src/hosts.rs`, `src/ledger.rs`, `src/repository.rs`, `src/close.rs` |
| Prerequisites | P1 and C1. C6 remains the scheduling prerequisite unless the exception is approved. |
| Excluded | Saving during close, retention, retirement, task settlement, automatic branch creation, setup hooks, and pane creation. |

### Public interface

```text
dogsled recovery list --repo <path> [--host <host>] [--json]
dogsled recovery show <id> --repo <path> [--host <host>] [--json]
dogsled recovery restore <id> --repo <path> --to <new-path>
                         [--host <host>] [--branch <new-name>] [--json]
```

Add equivalent MCP operations through the same runtime implementation. List and show are read-only. Restore creates resources but never replaces an existing checkout.

The repository and destination paths belong to the selected host. Default to the local host. Do not search the fleet implicitly.

## Writer instructions

### Snapshot reader and verification

Separate two contracts inside `preservation`:

| Operation | Required meaning |
|---|---|
| Read and verify archived snapshot | Verify the exact ref, commit metadata, parent structure, HEAD ancestry, tree identities, and raw-byte format. No original checkout is required. |
| Verify unchanged live checkout | Perform archived verification, then compare live HEAD, index, working bytes, modes, and supported path types. Preserve P1’s existing safety. |

Keep `snapshot` and its existing callers non-destructive.

Enumerate only `refs/dogsled/recovery/`. Treat IDs as IDs, not arbitrary revision expressions. Show malformed entries with a reason. Refuse their restoration before destination creation.

Capture the selected ref OID once. Revalidate that OID before restoration publishes success. A changed ref is a conflict, not a new expectation.

### Restore

Create an absent destination under the repository lifecycle lock. Refuse an existing directory, file, symlink, registered checkout, or replaced parent identity.

Restore these independent facts:

| Fact | Required result |
|---|---|
| HEAD | The saved commit, detached by default. Do not move the original branch. |
| Index | Exactly the saved index tree. |
| Working content | Exactly the saved raw bytes, deletions, symlink targets, and executable modes. |
| Untracked content | Remains untracked because it exists only in the working tree, not the restored index. |
| Original branch name | Reported as recovery metadata, not recreated implicitly. |

For an unborn snapshot, require an explicit unused `--branch`. Do not invent a branch name.

Add a bounded, byte-preserving subprocess result operation to the existing `Shell` seam. Use it for raw Git object reads. Do not decode blob contents through `String`, use filters, or bypass the shell’s process-budget handling.

Restore does not run repository setup, checkout hooks, agent startup, reservation, preparation, or teardown. It never pushes or expires the recovery ref.

On partial failure, retain the pinned recovery and report the exact destination. Do not recursively remove a destination whose identity changed.

## Interfaces consumed

| Interface | P2 use |
|---|---|
| P1 `snapshot` / `verify` | Reuse the persisted format. Split archived verification from unchanged-live verification without weakening either. |
| C1 lifecycle lock and identity | Protect destination creation and repository identity. Compare captured identities instead of trusting paths. |
| C3 `prepare::disposition::dispose` | **Not called.** Restore neither releases nor transfers an existing claim. |
| C4 placement phases | **Not entered.** Restore neither resumes placement nor creates an admission. |
| C6 retirement | **No callable C6 interface is required.** |

### Exactly what P2 needs from C6

P2 needs this invariant, not a retirement API:

> Restoration creates an independent checkout. It never clears, recreates, adopts, or resumes the original preparation claim, disposition, admission, grant, or task binding.

C6 must leave recovery refs untouched. Retirement must remain resource-only, with unknown execution outcome and prohibited future dispatch.

**P2 can proceed before C6 if this boundary is fixed now and the user approves the scheduling change.** Reusing `prepare::local` for restore would invalidate that exception. That function creates managed resources and preparation ownership: `src/prepare.rs:local_with:867–974`.

## Caller paths

| Path | Required routing |
|---|---|
| Public CLI list/show/restore | `Command::Recovery → runtime → preservation` |
| MCP list/show/restore | MCP wrapper → same runtime operations |
| Remote operation | Runtime → explicit host recovery command → same preservation owner |
| Existing P1 tests/callers | Continue through `snapshot` and unchanged-live verification |
| Future P3 result | Uses the returned recovery identity for list/show/restore. No separate reader. |

## Failing-first tests

Add these names under `tests/cli/preservation.rs`, except the explicitly named unit test.

| Test | Exact assertions |
|---|---|
| `p2_list_and_show_survive_original_checkout_removal` | List contains the saved ID after removal and process restart. Show returns the original ref OID, HEAD, index tree, and working tree. No preparation, placement, or launch record changes. |
| `p2_restore_preserves_index_worktree_and_raw_bytes` | Restored index tree equals the saved tree. File bytes and modes match exactly. Cover staged-plus-unstaged edits, binary/non-UTF-8 bytes, rename, deletion, executable file, symlink, and untracked file. Original checkout remains unchanged. |
| `p2_restore_does_not_run_filters_or_setup` | Configure observable smudge/setup hooks. Restore succeeds with raw saved bytes. Both hook counters remain zero. |
| `p2_restore_never_reuses_an_existing_destination` | File, directory, symlink, existing worktree, and equivalent-path destinations all refuse. Existing content, refs, and worktree registration remain unchanged. |
| `p2_restore_handles_detached_and_unborn_heads_explicitly` | Detached recovery creates no branch. Unborn recovery without `--branch` refuses before creation. Explicit unused branch preserves unborn HEAD and both trees. Existing branch refuses. |
| `p2_corrupt_or_changed_recovery_ref_refuses_before_materialization` | Malformed metadata, missing objects, wrong parent/tree, invalid ID, and ref replacement refuse. No destination checkout appears. List reports the invalid entry’s reason. |
| `p2_remote_restore_uses_selected_host_only` | Only the selected host creates the destination. Origin repository, refs, preparation claims, and admissions remain unchanged. Unsupported peer returns a clear refusal without fallback. |
| `p2_recovery_refs_remain_local_and_pinned` | Normal branch publication does not publish the recovery namespace. Restart and service retention leave every recovery OID unchanged. |
| `preservation::restore_rechecks_destination_identity` | A deterministic replacement between capture and materialization survives unchanged. The operation refuses rather than adopting it. |

## Acceptance

Run from the writer’s repository root:

```text
cargo test --bin dogsled preservation::
cargo test --test cli preservation::p2_
cargo test --test cli mcp::
cargo fmt --check
```

Expected: all pass, with nonzero selected test counts.

**Lock order:** Git lifecycle lock → short recovery-operation writes. No admission, preparation, done, or launch lock is necessary.

**Risks:** Lossy blob transport, filter execution, original-checkout dependence, destination symlink traversal, and accidental preparation ownership.

**User decisions with recommended defaults:**

| Decision | Recommended default |
|---|---|
| Start before C6 | Approve only with the independent-checkout boundary stated above. Otherwise retain the existing prerequisite. |
| Restore branch policy | Detached for committed snapshots. Explicit unused branch for unborn snapshots. This preserves content without changing existing branches. |
| Remote behavior | Explicit host only. Do not copy recovery objects to another host or push recovery refs. |

---

# P3 — Save on owner close and orphan close

**Cause:** Close rejects dirty content, and the shared remover requires a clean checkout even after teardown, so P1 snapshots alone cannot enable safe abandonment closure.

## Evidence

| Source at d7c7303 | Finding |
|---|---|
| `src/close.rs:with_local:188–290` | Close captures checkout identity before locking and holds repository protection through reconciliation. |
| `src/close.rs:remove_target:451–561` | The close path checks ownership, holders, loss, and then calls the shared remover. |
| `src/close.rs:unsaved:626–667` | Automatic close explicitly rejects dirty paths. All close modes require the current loss assessment. |
| `src/worktree.rs:remove:928–964` | Teardown runs before a second loss check and ordinary, unforced Git removal. |
| `src/worktree.rs:recheck_loss:1041–1052` | Teardown-generated unsaved content keeps the checkout and panes. |
| `src/sweep.rs:settle_stopped:485–559` | L3 saves the last reply, but dirty orphan closure remains blocked pending P3. |
| `src/items.rs:orphan_closed:177–195` | Resource-only closure already exists separately from task settlement. |
| `src/gate.rs:notices:121–125` | The current notice tells the owner to ask the user before discarding abandoned work. |

## Deliverable and scope

**Deliverable:** Eligible owner and stopped-orphan closes save recoverable content before removal and report `saved (recovery <id>)`.

**Suggested branch:** `feat/preserve-on-close`.

| Scope | Paths |
|---|---|
| Owned preservation/removal | `src/preservation.rs`, `src/close.rs`, `src/worktree.rs`, `src/loss.rs` |
| Owned orchestration/projection | `src/sweep.rs`, `src/items.rs`, `src/gate.rs`, `src/runtime.rs`, `src/main.rs`, `src/cli.rs`, `src/mcp.rs`, `src/sidebar/mod.rs`, `src/board.rs` |
| Owned tests | `tests/cli/preservation.rs`, `tests/cli/sweep.rs`, `tests/cli/close.rs`, `tests/cli/done.rs`, `tests/cli/mcp.rs`, `src/loss_tests.rs` |
| Read | `src/cleanup.rs`, `src/done.rs`, `src/launch.rs`, `src/launch/recovery.rs`, `src/custody.rs`, `src/prepare.rs`, `src/prepare/disposition.rs`, `src/unit_request.rs`, landed R1 owners |
| Prerequisites | P2, L3, R1, and their landed C6/RV prerequisites |
| Excluded | Task settlement, dirty `done`, preservation during ordinary prune, process killing, and legacy requestless retirement |

## Writer instructions

### Eligibility and authority

Enable preservation only for explicit owner close and positively identified stopped-orphan close.

Keep these protections:

| Protection | Required behavior |
|---|---|
| Managed checkout | Require `agent:worktree`. Preserve caller, main-checkout, foreign-lock, and outside-managed-directory safeguards. |
| Activity | Refuse live activity, external process holds, shared ownership, and unreadable evidence. |
| Orphan identity | Compare the captured session and turn against fresh stopped evidence. |
| Preparation ownership | A snapshot does not release a hold. Report the matching request and retirement action. |
| Task outcome | Saving or removing resources never creates native done, terminal return, or successful unit settlement. |

Ordinary native handoff remains clean-only. `Guard::Writer`, `Guard::Reader`, and ordinary done-based `Guard::Pushed` must not inherit preservation authority.

### Durable close transaction

Add a host-owned preservation operation record. Bind it to the captured item/request, pane terminal, checkout identity, HEAD, close mode, and recovery IDs.

Capture that expectation once. A retry reads it. A changed pane, checkout generation, item, or close mode refuses.

Implement this ordered transaction:

| Stage | Required behavior |
|---|---|
| Intent | Persist the operation and reserved recovery identity before snapshot construction. |
| Initial snapshot | Capture HEAD, index, working content, and untracked content. Pin the ref with CAS. Verify the ref and unchanged live checkout. |
| Teardown | Record entry before running teardown. A failed or uncertain teardown retains the checkout and visible reason. |
| Post-teardown capture | Recheck identity. If source changed, create and verify a successor snapshot before removal. Keep the earlier ref too. |
| Removal and reconciliation | Normalize only verified captured content for unforced removal. Remove through the existing owner. Persist the recovery result before reporting closure. |

P1 currently chooses its ID inside `snapshot`: `src/preservation.rs:snapshot:667–696`. Add an internal capture entry point that accepts the operation’s reserved ID and recovery context. Do not rewrite an existing pinned ref.

A snapshot must preserve deletion-only work too. Do not rely on a loss classifier that excludes held deletions: `src/loss.rs:lossy_paths:209–213`.

### Shared removal and loss proof

Do not merely bypass `unsaved`.

Extend the shared removal contract with explicit verified recovery evidence. The proof must name the exact checkout identity, HEAD, ref OID, index tree, and working tree.

After successful teardown and final verification, normalize only content represented by that proof. This permits ordinary Git removal of a formerly dirty checkout.

Do not use forced worktree removal, blanket `git clean`, or recursive checkout deletion. Ignored files retain Git’s ordinary unforced-removal behavior.

A recovery ref can prove that a detached HEAD is held. It must not make a retained branch “merged.” Keep branch disposition separate.

If normalization or removal fails, retain the recovery refs and operation record. Report whether the live checkout remains original, partially normalized, or removed. Never claim that failed teardown preserved unchanged live content unless verified.

A crash during teardown has an unknown hook outcome. Do not rerun the hook automatically. Report that reason and the preserved recovery identity.

### Result publication

Add a typed recovery reference to `Closed` and `Item`. Include host, repository, recovery ID, pinned OID, and predecessor IDs when teardown created another snapshot.

Persist the reference before closing the item. A remote lost reply must replay the host’s saved operation result, not snapshot a replacement at the old path.

Store enough context with the recovery record to retain its meaning after eligible item metadata expires. Do not make recovery accessibility depend permanently on `items.json`.

Use the existing resource-only orphan projection. Preserve the last reply through `orphan_reply`, not `set_reply`.

Show `saved (recovery <id>)` in close output and result/history reads. A snapshot alone never enters READY TO MERGE. An independently retained branch can still appear there.

Replace the abandoned-dirty notice with the native close-and-save action. Unsupported content still names its refusal reason. Do not replace it with routine requests for user file classification.

## Interfaces consumed

| Interface | P3 use |
|---|---|
| P1/P2 preservation | Capture, verify unchanged live content, verify archived refs, and provide native recovery commands. |
| C1 close capture | Preserve `CloseObservation`, saved launch identity, directory pins, `Tree::drift`, and verified absence. |
| C3 disposition | Consume ownership status. Never manufacture a disposition to make close succeed. |
| C4 placement | Pending placement/release remains a hold or explicit blocker. A saved snapshot does not complete placement. |
| C6 retirement | Offer the request-scoped retirement action for a held abandoned preparation. Retirement remains separately authorized and does not remove the checkout. |
| R1 | Retain unfinished preservation operations. Expire finished operation metadata only after required projections are durable. Recovery refs never expire. |

**Lock order:** Capture identity → lifecycle lock → existing done/launch locks where required → short preservation/items writes.

Do not call `dispose` or retirement while holding the lifecycle lock. Those operations acquire preparation protection outside it. Return the actionable hold instead.

## Every close caller

| Caller | P3 behavior |
|---|---|
| `src/runtime.rs:close:252–277` | Explicit owner close, local and remote. Persist the recovery result before item closure. |
| `src/mcp.rs:close:366–375` | Reuse runtime close. Update the dirty-refusal tool description. |
| `src/sidebar/mod.rs:Effect::Close:145–156` | Route through the reconciled owner-close path. The current direct call must not discard recovery metadata. |
| `src/main.rs:child:556–580` | Host owner-close and stopped-orphan transport. Return replayable recovery metadata. |
| `src/sweep.rs:settle_stopped:485–545` | Preserve reply first. Perform resource-only close for unresolved units. |
| `src/sweep.rs:close:755–772` | Preserve guard distinctions. Genuine done-based automatic closure stays clean-only. |
| `src/done.rs:close_mark:875–893` | Negative control: native handoff gains no dirty-preservation bypass. |
| `src/worktree.rs:remove:908` callers | Ordinary cleanup remains governed by its existing loss proof. Only explicit verified recovery evidence enables normalization. |
| `src/launch/recovery.rs:close_selected:280–307` | Stranded-launch shell closure continues to retain the checkout. Do not silently convert it into abandonment removal. |

## Failing-first tests

| Test | Exact assertions |
|---|---|
| `preservation::p3_owner_close_saves_before_removal` | Dirty eligible child closes. Recovery ref exists before the first content-clearing effect. Item and CLI result carry the same ID. P2 restore reproduces the saved HEAD, index, bytes, and modes. |
| `preservation::p3_deletion_only_close_preserves_deletions` | Saved index and working trees retain staged and unstaged deletions. Restore reproduces them even when default/remote refs hold HEAD. |
| `preservation::p3_snapshot_failure_keeps_live_work` | Inject intent, object, ref-pin, and verification failures separately. Before verified pin, no content clears and no pane closes. Live content remains unchanged. |
| `preservation::p3_crash_boundaries_keep_a_recoverable_copy` | Crash before pin, after pin, before normalization, and after removal. Every restart either retains original content or exposes a valid pinned recovery. No replacement checkout is removed. |
| `preservation::p3_teardown_changes_require_a_second_snapshot` | Hook changes tracked/untracked content. Hook runs once. Both refs survive. Latest recovery restores post-hook content before removal succeeds. |
| `preservation::p3_failed_or_unknown_teardown_keeps_checkout` | Failed hook or crash during hook leaves checkout retained. No automatic hook retry occurs. Item/status names the unknown or failed teardown and recovery ID. |
| `preservation::p3_unsupported_content_reports_a_hold` | Conflicted index, dirty submodule, nested repository, unreadable path, and unsupported type each retain checkout and panes. Each reports the specific reason. |
| `preservation::p3_replacement_or_head_drift_never_adopts_new_work` | Replace the checkout or move HEAD between capture and lock acquisition. Close refuses. Replacement bytes, refs, panes, and ownership records remain unchanged. |
| `sweep::p3_dirty_stopped_orphan_is_saved_without_task_completion` | Reply is durable before close. Recovery ID is recorded. Checkout is removed. Native done remains false. Unit settlement/binding remains unresolved. |
| `sweep::p3_nonstopped_or_uncertain_orphan_is_not_saved_or_closed` | Working, waiting, asking, blocked, finishing, lost, and unknown cases create no recovery and close no resource. |
| `preservation::p3_remote_lost_close_reply_replays_saved_recovery` | Retry returns the same operation and recovery IDs. Teardown runs once. A replacement at the old path survives. |
| `done::p3_dirty_done_still_refuses` | Native done refuses dirty content without producing recovery or terminal evidence. |
| `preservation::p3_saved_result_is_not_ready_to_merge` | Snapshot-only result reads as saved recovery and does not create a READY TO MERGE row. A separate retained branch keeps its existing classification. |

## Acceptance

```text
cargo test --test cli preservation::
cargo test --test cli sweep::
cargo test --test cli close::
cargo test --test cli done::
cargo test --test cli mcp::
cargo test --bin dogsled loss_tests::
cargo test --bin dogsled board::
cargo fmt --check
```

**Risks:** Destructive normalization before verification, teardown crash replay, lost remote result publication, shared-remover policy leakage, and recovery references that depend on expiring items.

**User decisions with recommended defaults:**

| Decision | Recommended default |
|---|---|
| Dirty-content normalization | Approve only after a pinned snapshot and final unchanged-content verification. Keep unforced Git removal. |
| Teardown changes | Keep immutable predecessor and successor recovery refs. Report the newest complete snapshot. |
| Unknown teardown outcome | Retain the checkout with a visible blocker. Require explicit inspection before another teardown attempt. |
| Retained branches | Preserve existing branch policy. A recovery snapshot is not branch integration or task completion. |

---

# RV — Non-job validity and immutable completion times

**Cause:** Validity enforcement explicitly bypasses agents and placements, preparation accepts requestless execution, and non-job owners lack immutable completion timestamps for safe retention.

## Evidence

| Source at d7c7303 | Finding |
|---|---|
| `src/runtime.rs:request_with_id:29–44` | Only jobs receive origin validity. |
| `src/work.rs:request_validity:254–268`, `check_request_validity:286–290` | Non-job requests and launches have no authoritative validity lookup or check. |
| `src/unit_request.rs:commit_using:237–249` | Commit replaces the agent action. Action-local validity can be lost here. |
| `src/node.rs:reserve:366–389` | Legacy reservation accepts absent validity. |
| `src/node.rs:reserve_placement:246–256`, `placement_locked:260–354` | Placement validates grant identity but does not enforce request age. |
| `src/prepare.rs:Envelope:385–391`, `local_with:876–907` | Preparation carries no validity. Requestless calls enter `local_once`. |
| `src/checkout.rs:ensure:90–111`, `local:205–213` | Repository provisioning can clone before preparation. It also needs a host-side boundary. |
| `src/launch.rs:agent_guarded:553–588`, `start_while_busy:122–155` | Agent allocation and repeated native start attempts have no validity check. |
| `src/jobs.rs:newest_record_ms:104–135` | Clock bootstrap scans job-specific validity locations. |
| `src/work.rs:Ticket:93–110`, `Attempt:1306–1320`; `src/prepare.rs:PreparationReceipt:1002–1017`; `src/launch.rs:Progress:26–43` | These owners have no `finished_ms`. |

## Deliverable and scope

**Deliverable:** Every agent, placement, and preparation effect requires immutable recorded validity. Owner-written completion times make later R1 deletion possible without changing replay evidence.

**Suggested branch:** `fix/nonjob-request-validity`.

| Scope | Paths |
|---|---|
| Owned validity/admission | `src/jobs.rs`, `src/error.rs`, `src/runtime.rs`, `src/work.rs`, `src/work/placement.rs`, `src/unit_request.rs` |
| Owned transport/effects | `src/commands.rs`, `src/hosts.rs`, `src/node.rs`, `src/checkout.rs`, `src/prepare.rs`, `src/prepare/disposition.rs`, `src/launch.rs`, `src/launch/recovery.rs`, `src/continuation.rs`, `src/cli.rs`, `src/main.rs`, `src/mcp.rs` |
| Owned terminal integration | `src/done.rs`, `src/items.rs`, `src/sweep.rs` |
| Owned tests | `tests/cli/request_validity.rs`, `tests/cli/place.rs`, `tests/cli/launch_retry.rs`, `tests/cli/unit_requests.rs`, `tests/cli/unit_seeded.rs`, `tests/cli/legacy_continue.rs`, `tests/cli/jobs.rs`, `tests/cli/mcp.rs`, `tests/cli.rs`, `src/work_tests.rs`, `src/work_tests/` |
| Mechanical fixture allowance | Existing Rust constructors and CLI wire fixtures affected by the new fields/protocol. No unrelated behavior changes. List every such path in the commit. |
| Read | `src/worktree.rs`, `src/ledger.rs`, `src/unit_transport.rs`, `src/unit_reply.rs`, `src/custody.rs`, `src/service.rs`, P2/P3 interfaces, C6 implementation |
| Prerequisites | C4b, C5, C6, and existing J2/C8 |
| Excluded | R1 deletion, remote reference-release protocol implementation, task completion policy, dependency changes, and automatic retirement |

RV must not overlap C5 or C6 writers. Reconcile the final cancellation and retirement transitions before dispatch.

## Writer instructions

### One validity authority

Reuse `jobs::Validity` and the existing persistent high-water clock. Do not create a second clock or different age policy.

Keep the exact J2 constants:

```text
Execution window: 2_592_000_000 ms
Allowed future skew: 300_000 ms
Expiry: effective_now - created_ms > window
Equality: still valid
```

Source: `src/jobs.rs:VALIDITY_WINDOW_MS:42`, `VALIDITY_SKEW_MS:44`, `check_validity_at:220–238`.

Add immutable validity to the non-job request and launch envelopes. Keep it outside `Action::Agent`, because unit commit replaces that action.

Use one authoritative sidecar per request. Extend the existing sidecar owner rather than adding competing copies. Preserve the current job wire and job sidecar semantics.

Only fresh origin issuance and validated first host ingress can publish authority. Existing queue, admission, preparation, or launch records cannot repair missing authority from an inline copy.

Capture once at issuance. Propagate unchanged through reservation, repository provisioning, preparation, launch, retry, unit commit, and recovery.

A contradictory stamp refuses. Never derive creation time from IDs, file times, queue retries, receipt arrival, or launch confirmation.

### Protocol boundary

Add an explicit non-job validity capability to host transport. Peers without it must refuse before capacity, clone, setup, pane, or agent effects.

Carry the immutable authority in preparation and checkout transport, not merely an origin-side check.

Version the preparation/placement envelopes where their interpretation changes. C6 and RV must agree on those versions before dispatch. Do not downgrade or silently retry an older effectful route.

Legacy records remain readable. A fully correlated retained final receipt can replay without new execution. Legacy missing validity never authorizes new preparation, provisioning, reservation, or start.

### Replay versus effects

Distinguish these operations:

| Operation | Expired request behavior |
|---|---|
| Return retained correlated final result | Allowed without allocation or dispatch |
| Read preparation outcome or placement history | Allowed read-only |
| Complete an existing exact release transaction | Allowed under C3/C6 correlation |
| Reserve new capacity, clone, prepare, allocate, start | Refused |
| Observe an already running matching execution | Allowed without restart |
| Continue an existing execution | Preserve existing ownership checks. Never turn missing launch/admission evidence into fresh execution authority. |

Do not put a blanket age refusal before a valid saved-result replay.

Recheck immediately before effects after blocking waits. In particular, check after repository/preparation locks, after preparation, and before **every** native agent-start attempt.

If expiry occurs after preparation, preserve the hold or release it only through existing conclusive no-start authority. Unknown execution remains unknown. Report the original request and C6 retirement action.

### Immutable `finished_ms`

Add `finished_ms: Option<u64>` outside immutable request, grant, binding, and result payloads.

Write it once under the record owner’s lock. A retry preserves the original value. A contradictory timestamp refuses retention eligibility.

Do not mutate `PlacementGrant` to add changing terminal metadata. Host replay compares the entire saved grant against the acknowledged grant: `src/node.rs:placement_locked:265–273`.

Use separate host-owned lifecycle metadata for grants, under the existing host/preparation gates.

| Owner | First permitted completion stamp |
|---|---|
| Queue ticket | Its final queue transition. Never pending/admitting or pending release. Ticket completion does not prove execution completion. |
| Placement admission | Completed result with validated release acknowledgement, or C5/C6’s durable terminal prohibition. Host selection and reservation TTL are not completion. |
| Ordinary admission | Correlated final refusal, closed execution, or other authoritative final outcome. A saved startup result is insufficient. |
| Unit admission | The recorded terminal settlement transition. Keep unresolved binding, custody, and reference predicates separate from this timestamp. |
| Preparation receipt | Once preparation ownership is conclusively settled or retired. A successful but held preparation remains unfinished for retention. |
| Disposition | Exact matching `Released` transition. Retirement can finish ownership with `Outcome::Unknown`. |
| Agent launch progress | Conclusive pre-start refusal, or positively proven terminal/closed execution. `confirmed` and terminal reply capture alone are insufficient. |
| Host grant lifecycle | After durable origin-terminal proof for that exact acknowledged grant. Never at hold expiry. |

For new transitions, use the owner’s effective high-water time. Record enough provenance to validate ordering.

Legacy missing timestamps remain missing unless an existing authoritative terminal timestamp can be copied exactly. Do not stamp “now” because a sweep first noticed an old terminal record.

Extend clock bootstrap to every added validity and terminal timestamp location. Corrupt/unreadable clock evidence fails closed.

### R1 handoff

Export owner-specific completion/validity predicates for R1. Do not implement deletion in RV.

The predicates must distinguish terminal outcome, resource release, and outstanding references. `finished_ms` alone is never deletion permission.

## Interfaces consumed

| Interface | RV use |
|---|---|
| P1/P2 preservation | No authority dependency. Read-only recovery and independent restore must remain possible after the original request expires. |
| C1 identity/lifecycle | Recheck validity after lifecycle-lock waits. Preserve captured checkout identity. |
| C3 `dispose` | Release only exact claims with supported policy and outcome. Expiry alone cannot produce `NotStarted`. |
| C4 phases | Preserve the original grant, preparation envelope, saved result, and acknowledgement through every transition. |
| C5 | Reuse cancel intent and possibly-executed classification. Do not equate failed/cancelled tickets with no launch. |
| C6 | Reuse durable future-dispatch prohibition and request-scoped retirement. Do not invent a second retirement path. |
| J2/C8 | Keep job behavior and current strict boundaries unchanged. |

## Every effectful caller path

| Path | Required checks |
|---|---|
| CLI agent/place and MCP legacy spawn/place | Stamp through `runtime::request` / `request_with_id`. |
| Prepared-unit prepare/commit | Stamp once during prepare. Preserve through action replacement. Correlated commit replay does not refresh it. |
| `submit`, `enqueue`, `enqueue_retry`, `perform`, `perform_locked` | Validate new work and existing authority. Saved-result replay remains distinct. |
| `unit_request::recover`, `work::admit`, `work::tick` | Recovery never reconstructs missing authority or restamps old work. |
| `commands::place_with`, `place_recorded`, `place_locked` | Validate before host selection and reservation effects. Preserve grant expectations. |
| `Fleet::reserve`, `Node::reserve`, `Node::reserve_placement`, host reserve | Validate at the host before writes. No absent-validity fallback. |
| `checkout::ensure`, `checkout::local`, host checkout | Carry authority and validate before clone/setup effects. This includes both C4 preparation paths. |
| `prepare::run`, `envelope`, `deliver`, `deliver_placement`, `local`, `on_host` | Propagate authority. Separate final outcome replay from fresh preparation. Refuse requestless effects. |
| `work::receive_launch`, `launch`, `launch_owned`, `launch_guarded` | Validate ingress and post-lock boundaries without replacing saved authority. |
| `launch::agent_guarded`, `allocate`, `start_or_reconcile`, `start_while_busy` | Check before allocation and each native start. Matching existing execution is observation, not restart. |
| Continuation and C3/C6 release | Preserve exact launch/binding correlation. Release operations do not require renewed execution validity. |

The repository-provisioning callers are `src/work.rs:prepare_launch:1496–1517` and `src/work/placement.rs:prepare_selected:365` / `recover_prepared:414`.

## Failing-first tests

Add the `rv_` tests to `tests/cli/request_validity.rs`, with owner-level timestamp tests beside their implementations.

| Test | Exact assertions |
|---|---|
| `rv_agent_and_place_stamp_once_at_origin` | Agent, place, and prepared-unit requests have one authority. Queue retry, commit, restart, and resume preserve its exact bytes. |
| `rv_missing_or_changed_authority_never_repairs_from_inline_data` | Delete or alter the sidecar while retaining each owning record. Retry refuses. The sidecar is not recreated. Capacity, setup, pane, and start counters remain zero. |
| `rv_expired_agent_and_place_refuse_at_origin_and_host` | At window+1ms, each public and direct-host effectful path refuses. No reservation, clone, preparation, pane, or agent-start effect occurs. |
| `rv_validity_boundaries_and_future_skew_are_exact` | Window equality is valid. Window+1ms refuses. Future skew equality is valid. Skew+1ms refuses. The original stamp never changes. |
| `rv_requestless_preparation_and_checkout_refuse` | Direct host preparation/provisioning without authority refuses before filesystem or Herdr changes. No claim, receipt intent, checkout, or pane appears. |
| `rv_expiry_during_preparation_prevents_agent_start` | Advance the clock after preparation. Native start count remains zero. Claim state follows C3/C5 proof, not an assumed no-start outcome. |
| `rv_expiry_during_busy_retry_prevents_another_start` | First native call returns busy. Advance the clock. No second native call occurs. Record preserves the original authority and a visible terminal/refusal reason. |
| `rv_expired_retained_receipts_replay_without_execution` | Agent final result, completed placement, and preparation lookup return the same retained outcome. Effect counters do not increase. Request/result bytes do not change. |
| `rv_deleted_replay_evidence_cannot_restart_old_requests` | Delete eligible historical evidence in the fixture. Replay the original expired wire through reserve, checkout, prepare, and launch. Every path refuses with zero effects. |
| `rv_old_peer_cannot_silently_bypass_validity` | Missing or unsupported capability refuses. Logs contain no older fallback reservation/preparation/launch request. |
| `rv_clock_bootstrap_includes_nonjob_records` | Remove the clock and roll wall time backward. Bootstrap reaches the newest authoritative non-job timestamp. Old requests remain expired. Corrupt/unwritable clock permits no effects. |
| `rv_finished_ms_is_write_once_and_not_startup_time` | Every owning terminal transition stamps once. Replay at a later clock preserves it. Selected grants, held preparations, confirmed-running agents, and uncertain execution retain no execution-finished stamp. |
| `rv_expired_refusal_preserves_actionable_ownership` | Expiry leaves unresolved ownership intact and reports its request-specific release action. A separately authorized C6 retirement releases only that claim. |

Keep these negative controls:

```text
interrupted_job_admission_preserves_running_record_without_relaunch
acknowledged_placement_missing_grant_never_allocates_after_ttl
placement_crash_boundaries_replay_one_result
a_correlated_replay_after_the_retention_pass_stays_read_only
```

## Acceptance

```text
cargo test --test cli request_validity::
cargo test --test cli place::
cargo test --test cli placement::
cargo test --test cli launch_retry::
cargo test --test cli unit_requests::
cargo test --test cli unit_seeded::
cargo test --test cli legacy_continue::
cargo test --test cli jobs::
cargo test --test cli mcp::
cargo test --bin dogsled prepare::
cargo test --bin dogsled work_tests::
cargo fmt --check
```

The lead owns the final repository gate, `moon.yml:check`, at the integrated revision.

**Lock order:** Use the common hierarchy. Advance and release the clock’s short lock without acquiring outer operation locks from inside it. Preserve `admissions → custody` for settlement acknowledgement: `src/unit_request.rs:ack_settlement:1014–1035`.

**Risks:** A missed clone/start boundary, validity lost during unit commit, blanket expiry blocking safe release, mutable timestamps inside grant identity, and startup timestamps misclassified as completion.

**User decisions with recommended defaults:**

| Decision | Recommended default |
|---|---|
| Validity owner | Reuse the current validity implementation, sidecar authority, and high-water clock. Avoid a parallel non-job clock. |
| Legacy execution | Read retained outcomes, but refuse new effects without provable authority. No migration-time creation stamps. |
| Completion migration | Preserve missing legacy completion times unless exact authoritative timestamps exist. Do not use observation time. |
| Already running work | Age prohibits new execution. It does not stop running work or remove unresolved ownership. |

---

# R1 decisions and required amendments

## Current prerequisite correction

The old #205 concern is resolved at this review base:

- `src/work.rs:expire_requests:172–185` restricts ticket expiry to jobs with conclusive local terminal evidence.
- `src/work.rs:expire_requests:207–222` retains validity sidecars while tickets or launch receipts still reference them.

R1 must extend these owners, not create another queue-expiry mechanism.

## Decision 1 — Remote reference-release proof

**Recommended default:** Require an explicit, versioned origin-release certificate for remote execution evidence. Host silence, owner absence, and settlement delivery are not release proof.

**Evidence:**

| Source | Meaning |
|---|---|
| `src/launch.rs:Progress:26–43` | The host records start/return/stop evidence, but no remote reference-release acknowledgement. |
| `src/unit_request.rs:Prepared:16–42` | Origin, settlement acknowledgement, custody acknowledgements, and reviews are separate facts. |
| `src/unit_request.rs:ack_settlement:1007–1035` | Settlement acknowledgement names an event and custody state. It does not assert that every execution/recovery reference is released. |
| `src/launch.rs:terminal_ack:1085–1122` | The host captures an immutable result, not proof that the origin no longer needs it. |

### Required certificate contract

The origin must capture and persist one release expectation containing:

```text
protocol version
origin host identity
execution identity
immutable launch/request identity
terminal evidence identity
custody revision and released reference set
```

The host compares that expectation against its retained records under the launch lock. It persists acceptance before deleting dependent evidence.

Identical replay succeeds. Changed identity refuses. Unreachable origin retains the evidence with `awaiting origin reference release` and a retryable synchronization action.

The origin must retain its release intent until the host acknowledgement is durable. Do not hold custody/items locks across the RPC.

**Ownership amendment:** R1C needs the release transport and schema owners, including `src/hosts.rs`, `src/node.rs`, `src/cli.rs`, `src/main.rs`, and `src/unit_request.rs`.

**Required tests:** `r1c_remote_release_requires_exact_execution_and_terminal_identity` and `r1c_lost_release_ack_replays_without_reopening_references`.

An acknowledgement from a task provider must explicitly release the required binding. Existing settlement acknowledgement must not be reinterpreted as that proof.

## Decision 2 — Expired-job identity retention

**Recommended default:** Keep the 30-day metadata bound and never create permanent job tombstones. Preserve precise expiry responses while a valid reference remains.

There is one product conflict to resolve before R1B deletes the last job identity:

> After every identity record is deleted, the service cannot distinguish a previously known expired job from a job that never existed.

**Evidence:**

| Source | Dependency |
|---|---|
| `src/jobs.rs:record_expired:587–594` | Missing Job records count as expired only while an item still names them. |
| `src/runtime.rs:read:305–346` | The item provides job type and host routing. Missing items fall back to agent resolution. |
| `src/runtime.rs:logs:620–647` | Remote expired-log handling also depends on an owner job item. |
| `tests/cli/jobs.rs:expired_item_rig:1622–1638` | Existing expiry tests deliberately retain the item. |
| `tests/cli/jobs.rs:expired_record_logs_report_expiry_to_the_owner:1660–1679` | Known expired jobs succeed, while unknown jobs return exit 2. |

### Recommended user disposition

Approve a bounded identity guarantee:

- While a valid owner/recovery reference exists, return the precise expired result using its immutable job ID and host.
- After the last eligible identity expires, return a typed unavailable result: `result unavailable: expired or unknown`.
- Never fall back from a missing job identity to an agent with a reused name or pane.
- Never recreate a Job record, extend its retention, or infer terminal proof from record absence.

This changes the unlimited interpretation of “the returning owner sees an expired result.” It needs explicit user approval.

If the user requires that exact distinction indefinitely, R1B must keep the relevant identity. That is a retention-policy exception, not an implementation detail.

**Required tests:** `r1b_expired_job_reference_keeps_exact_host_routing`, `r1b_last_job_identity_expiry_never_resolves_a_reused_agent`, and `r1b_job_expiry_does_not_wait_for_owner_ack`.

## Decision 3 — Missing-item notification race

**Recommended default:** A child event whose subject disappeared must refuse delivery. It must never become an unscoped owner-pane notification.

**Evidence:**

| Source | Race |
|---|---|
| `src/sweep.rs:tell:564–595` | Sweep copies items, then sends notifications afterward. |
| `src/inbox.rs:subject:1148–1162` | Missing and ambiguous subjects both return `None`. |
| `src/inbox.rs:custodian_of_event:1172–1178` | `None` becomes the supplied owner pane with no session. |
| `src/inbox.rs:holds:1100–1107` | Missing recorded session permits any live session on that pane. |

### Required repair

Separate event kinds at the inbox boundary:

```text
Child event: explicit subject and identity are required.
Non-child event: existing pane-only behavior remains permitted.
```

Pass a typed captured child identity rather than recovering authority solely from an event-name prefix.

Revalidate subject and custody before the inbox write. Missing, ambiguous, changed, or expired child identity produces a visible undelivered/superseded reason and no delivery.

Include reply, unit-reply, blocked, and signal events. Do not fix only ordinary reply IDs.

**Ownership amendment:** Add `src/inbox.rs` and its tests to R1B. Update affected callers in `src/sweep.rs` and `src/items.rs`.

**Required tests:** `r1b_item_expiry_between_tell_and_notify_never_delivers_to_reused_owner` and `r1b_nonchild_notifications_keep_existing_delivery`.

## C4a carry-over — Terminalize the origin before grant expiry

**Mandatory R1 requirement:** Before deleting a host placement-grants record, make the origin’s acknowledged `HostSelected` grant durably terminal.

**Evidence:**

| Source | Safety dependency |
|---|---|
| `src/work/placement.rs:PlacementProgress::expectation:105–115` | Acknowledged phases carry the saved grant as `expected_grant`. |
| `src/node.rs:placement_locked:265–273` | Missing or changed expected grants refuse. |
| `src/node.rs:placement_locked:295–334` | Without an expected grant or saved grant, the host can evaluate capacity and create a new grant. |
| `src/node.rs:reserve:374–388` | The legacy reservation route currently relies partly on the grant record’s presence to reject bypass. |

### Required ordering

```text
Origin admission lock:
  persist terminal state for the exact acknowledged grant
  prohibit further selection/preparation/start
  retain the immutable grant identity

Host:
  receive and validate origin-terminal proof
  verify ownership/reference release
  verify strict age expiry under RV’s persistent clock
  delete the grant through its owning store
```

Do not let an offline origin, elapsed reservation TTL, missing preparation, or failed ticket substitute for terminal proof.

R1 must also cover `SelectionPending` requests whose host allocated a grant before the reply reached the origin. Missing acknowledgement is not permission to delete uncertain ownership.

After grant deletion, both routes must remain safe:

| Replay | Required result |
|---|---|
| Original request with `expected_grant` | Missing-grant refusal or retained terminal result. No new capacity. |
| Original request without `expected_grant` | RV age/protocol refusal before capacity evaluation or writes. No new capacity. |

Add `r1a_grant_expiry_requires_origin_terminal_proof` and `r1a_deleted_grant_replay_without_expectation_never_allocates`.

Assert no grant recreation, no new hold, unchanged reservation count, zero preparation/setup/pane effects, and no origin transition back to selection.

---

# Lead dispatch checklist

| Unit | Release condition |
|---|---|
| P2 | Bind P1/C1 SHA. Record the restore defaults and either C6 completion or the approved scheduling exception. |
| RV | Bind landed C5/C6 SHA. Freeze protocol versions and terminal predicates. Serialize all shared writers. |
| R1 | Require RV. Resolve the three decisions and add the grant-terminalization requirement before deletion work starts. |

[Showing lines 1-764 of 768 (50.0KB limit). Use offset=765 to continue.]