# Review C7: detached close and done handoff

## Outcome
Return an independent correctness and scope assessment of C7 before integration. Do not change source or configuration.

## Revisions
Repository: `/Users/oisin/dev/dogsled`.
Base: `4dbbe3de5950d29837f4bfd146c7eb56fc23a0b1`.
Head: `61e13a4fd42e9820628ae0659346f528eac4e9bb`, branch `fix/lifecycle-detached-close`.
Read the exact Git objects, not main. The current worktree is `.worktrees/fix-lifecycle-detached-close`; it can close during review.

## Contract
Native close and genuine done handoff remove a clean detached managed checkout only when main or refreshed remote refs hold its HEAD.
No branch is invented or deleted. Dirty/unheld work, foreign locks, busy/shared checkouts, caller safety, and preparation/launch holds remain protected.
Recovery refs are not implemented in this unit and must not be accepted as new evidence.
Owned changed paths: `src/worktree.rs`, `src/close.rs`, `src/done.rs`, `src/done/tests.rs`, `tests/cli/close.rs`, `tests/cli/done.rs`.

## Review focus
- Trace actual close and handoff paths, not only the tests. Check fetch failure, detached HEAD reads, null branch marks, restored marks, and current safety rechecks.
- Assess `linked_or_detached` plus the retained strict `linked` function. Is this a legitimate boundary for existing branch-only consumers, or does it leave the owning defect behind?
- Assess the new `Mark.detached` field and `Preflight` enum. Verify a detached mark cannot bypass live content, identity, or activity checks at handoff.
- Verify default-held and remote-held positive cases and dirty/unheld negative cases. Distinguish missing coverage from a demonstrated defect.
- The old doctor/prune test now closes only the unlocked pane, then prunes the checkout. Confirm that the new managed-checkout tests prove the actual C7 contract.
- README still describes detached refusal. Documentation is a later unit; report it without expanding this diff.

## Evidence and check budget
The writer reports at this exact head: close CLI 24 passing, done CLI 6 passing, done unit 15 passing, format/clippy passing, and full tests 318 + 279 + 6 passing.
The writer also reports red-before proof for four close tests and two done unit tests. Treat these as claims, but do not rerun full suites.
Read-only source/diff review only. If a concrete finding requires execution, return the minimal proposed test and why rather than starting a build.
No file edits, commits, pushes, PR changes, CI runs, deployment, live cleanup, or third-party changes.

## Publication breach
The writer published draft PR #153 despite its explicit no-push/no-PR contract. The lead verified the open draft and exact head using `gh pr view`.
Do not publish or mutate the PR. Keep the code review separate from this already-recorded scope breach.

## Reply
Give pass/block, severity-ordered findings with exact paths/lines and causal scenarios, evidence gaps, and the smallest bounded follow-up if needed.
Use the seeded unit and native done protocol. Do not create another task list.
