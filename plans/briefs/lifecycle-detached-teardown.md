# C7 follow-up: recheck loss after teardown

## Outcome and cause
Close and genuine done handoff must retain the checkout and panes when teardown creates unheld detached commits or unique uncommitted content.
The owning defect is `worktree::remove`: it checks only deletion restoration after teardown, then closes panes and removes the checkout. A clean new detached commit bypasses that check.

## Revisions and prerequisites
Repository: `/Users/oisin/dev/dogsled`.
Reuse retained branch `fix/lifecycle-detached-close` WITHOUT a new base argument. Its current head is `61e13a4fd42e9820628ae0659346f528eac4e9bb`.
The original writer has closed. Preserve all its commits. Do not reset, recreate, or rebase the branch.
Prerequisite: independent review unit `u-348e6f3b-9dcf-494f-b3ad-17bce4ba010e-53` returned BLOCK at that SHA. The lead confirmed the causal path in source.
C7 is not integrated or accepted. Do not report the original revision as safe.

## Owned and read paths
Owned: `src/worktree.rs`, `tests/cli/close.rs`, `tests/cli/done.rs`.
Generated: none.
Read: applicable local instructions and code-standard references; `src/close.rs`, `src/done.rs`, `src/loss.rs`, `src/loss_tests.rs`, `src/cleanup.rs`, `src/repository.rs`, and the owned tests.
The concurrent J1 writer owns jobs/runtime/work and job fixtures. Do not edit its paths or the shared main checkout.

## Required repair
- Put the final loss assessment in the shared removal owner, after repository teardown and before any holder pane/workspace close, unlock, or checkout removal.
- Read the checkout's CURRENT HEAD and assess current content with the existing `loss::Evidence`. Do not rely on the pre-teardown HEAD or cleanliness alone.
- Keep existing held-deletion restoration behavior. A deletion-only checkout may recover held files before ordinary unforced removal.
- Unheld HEAD, unique dirty content, unreadable HEAD, or failed assessment must refuse explicitly and retain the checkout, its managed lock, panes, and unclosed item.
- Use the shared rule rather than a detached-only flag, retry, sleep, or special-case test input. Keep named-branch retention valid for the callers that retain that branch.
- Do not add preservation snapshots, recovery refs, new lifetime state, repository-lock redesign, deployment behavior, or a workaround in callers. Those are different units.

## Regression proof
First add deterministic regressions on `61e13a4`:
1. Explicit close starts with a clean held detached HEAD. A real teardown hook creates and commits new content. Prove close refuses and the new commit, checkout, lock, panes, and open item remain.
2. Genuine native done is marked while detached HEAD is safely held. The handoff teardown creates and commits new content. Prove no handoff closure is recorded and panes/checkout remain. Do not claim the final task reply must be erased: task handoff and resource closure are different facts.
3. Protect the shared boundary with a focused case for teardown-created unique dirt, and preserve existing safe close/handoff and deletion-recovery tests.
Use repository fixture hooks and real Git; no sleeps or weakened existing assertions. Keep actual current HEAD reachable through the retained checkout after refusal.
The missing real-remote refresh/fetch-failure coverage is a separate review gap; do not expand this repair into a new transport suite.

## Acceptance and check budget
In the worktree run:
`cargo test --locked --test cli close:: && cargo test --locked --test cli done:: && cargo test --locked --bin dogsled loss_tests:: && cargo test --locked --bin dogsled done::`
Also allow scoped format checks and required commit hooks (`mise run check:rust`). Do not bypass hooks or run repeated full suites outside required hooks.
Report red-before and green-after exit status, exact commands, and exact final SHA. Preserve the actual Cargo status when output is piped.

## Authority and handoff
Allow one scoped LOCAL commit on the retained branch, with all existing commits preserved.
NO push, PR creation/update, merge, release, deployment, live fleet cleanup, dependency changes, or edits outside ownership. The existing draft PR #153 was unauthorized; do not touch it or update its remote branch.
Stop and report a scope blocker instead of broadening. Return branch, SHA, changed paths, causal repair, test evidence, and remaining gaps. Use the seeded unit and native done protocol; do not create another task list.
