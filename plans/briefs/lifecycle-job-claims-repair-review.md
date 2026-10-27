# Review J1 repair: exact claims and proven replay

## Outcome
Independently decide whether J1 can be integrated. Read-only.

## Revisions
Repository: `/Users/oisin/dev/dogsled`. Branch `fix/lifecycle-job-claims`.
Base `4dbbe3de5950d29837f4bfd146c7eb56fc23a0b1`. Blocked J1 head `1a965411b2430e4f3dc9abf1c305e120bd1ac778`. Repair head `7feffe978b515ae4a9f62e85fed2ae0617bcfd2e` (one commit).
Review the whole `4dbbe3d..7feffe9`, focusing on `1a96541..7feffe9`. Read exact Git objects; the linked worktree was removed at handoff.
Contracts: `plans/briefs/lifecycle-job-claims.md`, `plans/briefs/lifecycle-job-claims-repair.md`. Prior BLOCK brief: `plans/briefs/lifecycle-job-claims-review.md`.
Local `main` now contains C7 (`cbaac83`). Check that the J1 branch merges cleanly against it and that no merged C7 behavior conflicts.

## Prior findings to verify closed
1. Embedded-ID adoption: `runtime::exec`, `jobs::ownership`, `jobs::pending_claim` and `jobs::exec` now bind the requested ID. Check all reach paths, including `stop`, `list`, `wait`, and node/snapshot readers, for any remaining filename-vs-embedded mismatch that matters.
2. Unproved replay: `work::launch_job` refuses any existing claim without a saved receipt, and `correlate` validates a saved receipt against the request and recorded job. Trace the consequences:
   - A crash after successful `pane run` but before the receipt write now refuses forever. Confirm admission/service then reports a terminal failure, not an endless retry, and nothing releases preparation ownership.
   - `correlate` refuses when the Job record is missing. Note how this interacts with future C8 Job-metadata expiry; that is a finding for C8, not a J1 defect, unless reachable now.
3. Complete pending invariants under the start lock: empty pane, result fields, identity, and changed immutable fields. Confirm the empty-pane-executes expectation was removed and no legitimate path now regresses.
4. README/design wording now says "no new" effects.

## Race proof
New tests (`launch_retry.rs` ~1345–1515) claim real-wrapper, FIFO-ordered dispatch orders. Verify they run the real `dogsled exec`, impose order through process barriers rather than sleeps, bound waits, and clean up processes. The older copied-JSON test remains; judge it only as an extra test.

## Other checks
Lock discipline: no execution lock inside a launch/store lock; short record lock does not span `pane run` or the command lifetime. Scope: eight changed paths, all owned. Minor: `work_tests.rs` new test leaks its first temp ledger dir.

## Evidence and limits
Writer reports: red before (jobs 3 failed, launch_retry 3 failed, exit 101); green acceptance chain exit 0 (jobs 14, lifecycle 6, launch_retry 41, review 1, work_tests 8); hook `check:rust` passed (318/289/6). Lead verified branch head, one commit, `git diff --check`, and that only the installed daemon `dogsled serve` remains running.
No builds or tests. Propose minimal regressions if needed. No edits, commits, pushes, PR changes (#155 is unauthorized; leave it), CI, deployment or cleanup.

## Reply
Pass or block, severity-ordered findings with file:line and causal scenarios, coverage gaps, and the smallest follow-up if needed. Use the seeded unit and native done.
