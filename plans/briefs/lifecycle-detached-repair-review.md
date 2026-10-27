# Review C7 with its teardown repair

## Outcome
Independently decide whether C7 can be integrated: detached close and done handoff, plus the post-teardown loss recheck. Read-only.

## Revisions
Repository: `/Users/oisin/dev/dogsled`. Branch `fix/lifecycle-detached-close`.
Original base: `4dbbe3de5950d29837f4bfd146c7eb56fc23a0b1`.
Blocked C7 head: `61e13a4fd42e9820628ae0659346f528eac4e9bb`.
Repair head: `8ada6ccc112a14bbbe8f4dee385cd6ee3a81b303`.
Review the whole `4dbbe3d..8ada6cc` change, with focus on `61e13a4..8ada6cc`. Read exact Git objects.
Contracts: `plans/briefs/lifecycle-detached-close.md`, `plans/briefs/lifecycle-detached-teardown.md`. Prior review: `plans/briefs/lifecycle-detached-review.md` and its BLOCK finding.

## Focus
1. `worktree::remove` now calls `recheck_loss` after teardown and before restoring deletions, closing panes, unlocking, or removing. Verify it uses current HEAD and current content, and refuses with checkout, lock, panes and item intact.
2. Evidence semantics per caller. Named-branch handoff uses `keeping_branch`, so a teardown commit on the retained branch counts as held. Explicit close and cleanup sweep/prune use fetched or other evidence. Trace `src/close.rs` and `src/cleanup.rs:1824` callers. Confirm no caller can now delete a branch or checkout holding unheld teardown work, and no safe path regressed.
3. Ordering with `restore_deletions`: deletion-only held checkouts must still recover and remove.
4. Regressions: two close tests and one done-handoff test, reported red-before. Check they exercise real teardown hooks and real Git and assert the right retained state.
5. Scope: `tests/cli/watchdog.rs` (+2 fake-git lines) is outside the owned set. Judge whether the fixture change is a faithful answer to the new Git calls, or masks behavior.
6. Remaining gaps from the first review: real remote refresh and fetch-failure coverage, README detached wording. Report them as gaps unless they now hide a defect.

## Evidence and limits
Writer reports at `8ada6cc`: pre-commit `mise run check:rust` passed (fmt, clippy, 318 bin + 282 cli + 6 plugin tests). Lead verified the branch head, a clean worktree, and `git diff --check`.
Do not rerun builds or tests. Propose a minimal regression if a finding needs execution.
No edits, commits, pushes, PR changes, CI, deployment, or cleanup. Draft PR #153 exists without authorization; leave it alone.

## Reply
Pass or block, severity-ordered findings with file and line references and causal scenarios, coverage gaps, and the smallest follow-up if needed. Use the seeded unit and native done.
