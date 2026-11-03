<!-- AC1 architect output (arch-ac1, 2026-10-28), reviewed base 24d620c. Lead decisions:
  - APPROVED: write-once dogsled-checkout-id generation marker in the worktree admin dir, paired with canonical gitdir path.
  - APPROVED: src/gate.rs fixture-only scope addition (Tree test literal at :482-487).
  - Writer base: origin/main after L2 (-17, PR #209) merges; recheck affected calls at that base. -->

# C1 — Repository lock and verified absence

**Cause:** Removal paths lack shared serialization, and checkout identity omits the Git administrative directory (`src/worktree.rs:344–423`, `src/cleanup.rs:18–46`).

## Base and dispatch conditions

Reviewed **`24d620cb18df3fc0b721a8407cd734649bb8e5fd`**, a descendant of `509b492`.
Two `git ls-remote origin refs/heads/main` reads matched this revision.
The available checkout was on Linux, not `/Users/oisin/dev/dogsled`.
I read committed objects without changing the checkout. The read-only architect restriction prohibited `git fetch`.
No files changed. No tests ran.

**Dispatch remains conditional:** integrate L2, approve the identity-generation detail below, and extend the mechanical fixture scope.
C7 removal and C8 retention exist at this base (`src/worktree.rs:361–367`, `src/jobs.rs:403–425`).
The requested L2 signal changes are absent from this base.
After L2 lands, record the exact writer base and recheck the affected calls.

**Owned:** `src/{worktree,prepare,launch,work,close,cleanup,done,loss_tests}.rs`, `tests/cli/launch_retry.rs`.
**Required scope addition:** `src/gate.rs:482–487`, solely to update its `Tree` test literal.
**Generated source:** none. **Publication authority:** this brief grants no push, PR, merge, or deployment authority.

## 1. Current paths and lock order

Arrows indicate nested locks. Separate stages release their locks before the next stage.

| Path | Current behavior and locks |
|---|---|
| Explicit close | `close::local → remove_target → worktree::remove` (`src/close.rs:91–100,128–227`). No encompassing lock. `done::finished` temporarily takes `done` (`src/done.rs:240–242,184`). Registration removal later takes `worktrees` (`src/cleanup.rs:1777–1782`). |
| Close entry points | Local runtime, host RPC, sidebar, and owner sweep call the same function (`src/runtime.rs:244`, `src/main.rs:543`, `src/sidebar/mod.rs:149`, `src/sweep.rs:683`). Runtime closes origin items afterward (`src/runtime.rs:254`). |
| Explicit prune | `runtime → cleanup::apply → apply_one → remove_tree` (`src/runtime.rs:427–447`, `src/cleanup.rs:2002–2008,2135–2137`). No encompassing cleanup lock. Revalidation temporarily takes `done` through `Seen::load` (`src/cleanup.rs:235–238`). |
| Automatic cleanup | `cleanup::sweep → apply_survey → apply_one` reaches the same removal (`src/cleanup.rs:2033–2067`). `sweep-request` does **not** span cleanup (`src/sweep.rs:175–187`). |
| Native done handoff | `handoff →` temporary `done` reads/writes, then close, then separate `done` and `items` writes (`src/done.rs:649–658,733–743`). The outer `handoff` lock does not coordinate with prune. |
| Shared physical removal | `worktree::remove` runs teardown, rechecks loss, closes holders/workspace, unlocks Git, removes the checkout, forgets registration, and handles branch/anchor cleanup (`src/worktree.rs:353–426`). Only registration forgetting takes a Dogsled store lock. `agent:worktree` is a Git retention marker, not this coordination lock. |
| Failed preparation rollback | `prepare::local → local_once → make → discard` (`src/prepare.rs:120–158,351–357,415–430,596–600`). Requestful calls hold `preparation-receipts/<id>`. Short `preparations` writes occur before setup and after failure. Requestless calls lack the receipt lock. `discard` directly runs forced removal without teardown. |
| Preparation release/retirement | Existing `prepare::used` removes only the matching preparation record under `preparations` (`src/prepare.rs:500–511`, `src/main.rs:655–657`). It does not remove a checkout. Planned `retire-preparation` is C6, not an existing removal route. |
| Launch recovery | `close_unfinished` and `close_final` take `launches/<id> → launch-panes/<pane>` (`src/launch/recovery.rs:271–278,316–345`). They close only a proven shell and retain checkout/evidence. Cleanup calls recovery before checkout removal (`src/cleanup.rs:1804–1832`). |
| Launch refusal/expiry | Agent refusal runs inside launch/pane locks (`src/launch.rs:390,417–425`). Expired job allocation retires only its allocated pane (`src/work.rs:567–573`). Neither removes a checkout. |
| Metadata-only cleanup | `drop_registration` takes `worktrees`; `drop_repository` takes `repositories → worktrees` (`src/cleanup.rs:1857–1888`). Neither deletes a checkout. Tagged-cache removal is separate (`src/cleanup.rs:1007–1015`). |

The comments mentioning a “cleanup lock” do not establish one (`src/cleanup.rs:2152,2199`).

## 2. One coordination primitive

Implement the primitive in `src/worktree.rs`, using existing `Ledger::with_lock` (`src/ledger.rs:76–104`).

**Mutex address:** `<canonical Git common directory>/dogsled-lifecycle.lock`.
Resolve the common directory with the existing helper (`src/worktree.rs:304–318`).
Carry the caller’s budget into the lock ledger.
Do not remove the lock file during cleanup.

**Operation identity:** `(common directory, canonical checkout root, HEAD, optional branch, gitdir identity)`.
The operation identity is **not** the mutex filename.
A HEAD change or replacement must still contend on the same repository mutex.
Per-request or HEAD-derived mutexes cannot serialize these operations.

Provide a guarded entry point and internal functions that accept the existing guard.
Do not reacquire the repository mutex inside `remove`, preparation rollback, or handoff callbacks.

```text
Existing outer service/admission-driver/handoff serializers, when applicable
→ origin admissions/<id>
→ host preparation-receipts/<id>
→ Git common-directory lifecycle
→ done
→ launches/<id>
→ launch-panes/<pane>
→ short store locks
```

**Custody:** keep it a short store operation. Existing settlement/review uses `admissions → custody` (`src/unit_request.rs:983–984,1009–1024,1071–1084`).
Never acquire the repository lock, admissions, or an RPC from inside custody.

**Queue:** keep its lock outside long operations. `admit` already releases it before `perform` (`src/work.rs:1267–1278,1317–1326`).
**Jobs:** repository protection precedes short job-record locks. C1 must not acquire the lifetime `jobs/<id>-exec` lock.
That existing wrapper takes `exec → job record → validity clock` (`src/jobs.rs:914–952`).
Wrapper fencing belongs to C2.

**Inversion checks:** preserve continuation’s `admissions → done → launches` (`src/continuation.rs:65–67`).
Acquire repository protection before entering launch receipt locks, never inside `with_legacy_unreturned`.
Keep `prepare::used` a leaf: callers already hold launch locks (`src/work.rs:641–679`, `src/launch.rs:953–1004`).
Keep `handoff` outermost and release temporary `done` locks before repository acquisition.

## 3. Same-resource identity and writer edits

1. **Identity owner:** extend `Tree` with optional administrative identity and extend `Linked` with the corresponding live identity.
   Capture HEAD, branch, common directory, and absolute `--git-dir` together.
   `linked_or_detached` currently reads then discards the administrative directory (`src/worktree.rs:109–143`).
   Branch-only cleanup entries retain no checkout identity (`src/cleanup.rs:1126–1131`).

2. **Generation detail requiring approval:** use a write-once `dogsled-checkout-id` inside the administrative directory.
   Pair its value with the canonical gitdir path. A directory pathname alone does not establish its incarnation.
   Create the identity under the repository mutex during preparation or fresh discovery, never during stale-request validation.
   Missing or malformed identity on a stale request must refuse. Do not silently bind that request to today’s checkout.

3. **Compare before effects:** extend `Tree::drift` and reuse `validate_tree`.
   Different repository, root, branch, gitdir, or generation means identity refusal.
   Same identity with moved HEAD means a skip, preserving current behavior (`src/cleanup.rs:1732–1747`).
   Recheck identity after teardown, before pane closure, unlock, deletion restoration, or removal.
   Preserve C7’s post-teardown loss assessment (`src/worktree.rs:362–367`).

4. **Critical sections:** protect close’s final target/ownership checks through reconciliation.
   Protect cleanup’s final validation through removal and registration reconciliation.
   Protect preparation selection, claim publication, create/reuse, setup, rollback, and registration.
   Move preparation registration inside that section: it currently happens afterward (`src/prepare.rs:75–84,463–474`).
   Protect `work::launch` effects before its launch locks. Save checkout identity in existing preparation/launch records.
   Keep successful receipt replay read-only. Do not add job-wrapper fencing or preparation disposition semantics.

5. **Handoff:** call the guarded close implementation without nested repository acquisition.
   Keep the guard until the matching closed mark and host-local item updates finish.
   Reuse saved preparation/launch identity when the checkout disappears before reconciliation.
   Preserve sealed replies and native-done provenance. Do not infer completion from checkout absence.
   Keep L2’s Mark/Signal/observe behavior unchanged. Origin-host reconciliation after RPC remains separate.

## 4. Verified absence

Define `Gone` only when all evidence agrees under the repository mutex:

- The original repository remains readable and matches its recorded common directory.
- A successful, **unfiltered** worktree listing contains neither the target path nor its administrative identity.
- `symlink_metadata(checkout_path)` returns `NotFound`.
- The recorded administrative directory is absent.

Use the raw porcelain listing, not `listed`, which filters prunable entries (`src/cleanup.rs:521–525`).
A prunable registration, dangling symlink, surviving directory, unreadable path, or failed Git command is not absence.
Reuse `gone`, `unless_gone`, and `validate_tree` with this predicate (`src/cleanup.rs:1658–1773`).

Route a requested checkout through identity/absence validation **before** generic “no longer in survey” rejection.
Current `apply_one` can reject before reaching `Recheck::Gone` (`src/cleanup.rs:2087–2124`).
After removal errors, return success only if the complete absence predicate passes.
Reconcile only matching records. Run no second teardown, pane close, unlock, or branch deletion.
An unknown pane without captured or saved checkout identity is not proof of successful removal.

## 5. Exact regression tests

Use real Git and controlled barriers/FIFOs, not scheduling sleeps.
Existing examples: `src/loss_tests.rs:1170–1204`, `tests/cli/launch_retry.rs:1770–1774`.
The assertions below are required outcomes. Their predicted baseline failures are source-traced, not executed.

| New test | Setup and red-before assertion |
|---|---|
| `loss_tests::concurrent_close_and_prune_run_one_teardown` | Eligible locked checkout, stateful panes, blocking teardown. Exercise both winners and attached/detached variants. Assert one teardown/removal and two successful outcomes. Baseline lacks shared serialization. |
| `loss_tests::prune_after_prior_removal_reconciles_without_failure` | Save a prune plan, remove the checkout and its registration, then call real `cleanup::apply`. Assert no failure and no teardown. Baseline rejects the missing survey entry. |
| `loss_tests::close_after_prior_removal_reconciles_without_teardown` | Capture the close target, block before its final check, then complete competing removal. Assert success and matching-record reconciliation. Baseline uses stale pane/tree observations. |
| `loss_tests::same_path_same_head_replacement_is_preserved` | Save a plan, replace the checkout with identical path/branch/HEAD but another administrative identity. Include reused gitdir-path generation. Assert replacement and registration remain, with zero teardown. Baseline `Tree::drift` sees equality. |
| `loss_tests::teardown_replacement_is_preserved` | Teardown substitutes a clean, held replacement. Assert replacement, panes, and matching new records remain. Baseline post-hook check assesses loss but not incarnation. |
| `loss_tests::absence_requires_git_disk_and_admin_agreement` | Parameterize missing disk with retained/prunable admin, failed listing, dangling symlink, and surviving unregistered content. Assert no reconciliation or deletion. The filtered-list absence case must fail before the fix. |
| `launch_retry::prepare_and_launch_wait_for_repository_lifecycle_lock` | Hold the proposed mutex from another process. Start host preparation and fresh agent/job launch separately. Assert no setup, allocation, claim, or dispatch before release. Baseline ignores this mutex. |
| `launch_retry::setup_failure_preserves_replacement_checkout` | Setup replaces its newly created checkout, then fails. Assert rollback refuses the replacement and retains its branch/content. Baseline `discard` removes by path with force. |
| `launch_retry::handoff_reconciles_removal_before_closed_mark_save` | Sealed native-done reply plus saved checkout identity. Force competing removal before handoff reconciliation. Assert closed mark/item, unchanged reply, and one teardown. Baseline can leave the mark open. |
| `launch_retry::repository_lock_precedes_launch_and_store_locks` | Hold the repository mutex and start preparation/launch contenders. Probe receipt/store locks at controlled boundaries. Assert the documented order and bounded completion after release. Baseline launch effects occur without repository protection. |

Retain existing HEAD-move, missing-`.git`, unreadable-index, detached-loss, and launch-replay tests.
Keep `interrupted_job_admission_preserves_running_record_without_relaunch` as a negative control (`tests/cli/launch_retry.rs:1223`).

Writer acceptance: `cargo test --test cli launch_retry::` and `cargo test --bin dogsled loss_tests::`.
Add focused existing close/done tests after their call paths change.
The lead owns final repository-wide gates. Do not bypass required hooks.

## 6. Risks, exclusions, and open decisions

The mutex coordinates Dogsled, not arbitrary Git commands or external writers. Fresh identity and loss checks remain necessary.
Setup/teardown spans the mutex. Recursive Dogsled lifecycle calls from hooks can exhaust the lock budget.
“Exactly once” covers competing successful removals, not crash-safe teardown execution. A crash journal is outside C1.
C2–C6 job fencing, dispositions, replay, cancellation, and retirement remain separate. Preservation implementation also remains separate.
**Before dispatch:** integrate L2, approve the administrative generation marker, and authorize the `src/gate.rs` fixture-only scope addition.