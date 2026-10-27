# Review J1: durable job claims

## Outcome
Independently assess fail-closed execution, host replay, and dispatch uncertainty at the exact J1 revision. Return pass/block with causal evidence and a bounded follow-up if needed. Do not edit source.

## Revisions and source
Repository: `/Users/oisin/dev/dogsled`.
Base: `4dbbe3de5950d29837f4bfd146c7eb56fc23a0b1`.
Head: `1a965411b2430e4f3dc9abf1c305e120bd1ac778`, retained branch `fix/lifecycle-job-claims`.
Read exact Git objects. A linked worktree may disappear after native handoff; do not rely on its continued presence or substitute main.
Read `/Users/oisin/dev/dogsled/plans/briefs/lifecycle-job-claims.md` for the approved contract and the applicable local/code-standard rules.

## Contract
The wrapper requires an existing exact executable pending claim under its lifetime execution lock. Missing, corrupt, conflicting, started, terminal, or uncertain evidence refuses before process/log/pane effects.
The host records non-executable intent before allocation and does not overwrite or redispatch an existing claim when a receipt is missing.
An ambiguous transport failure preserves concurrently published running/terminal evidence, or marks unproved startup uncertain without fabricating an end.
The short record lock does not span pane submission or command lifetime. The execution lock is not acquired inside a launch or store lock.
J2 request age and C8 retention are not implemented here; do not demand their future behavior or approve deletion based on J1 alone.

## Exact review focus
1. **Claim identity and contents.** `record` loads by filename, while `runtime::exec` passes the saved Job into `jobs::exec`, which locks/loads using that Job's embedded ID. Verify mismatched embedded ID cannot authorize another pending job. Assess command/options/cwd/host/owner/pane correlation, contradictory terminal fields, empty-pane claims, and revalidation under the start lock. Separate actual reachable defects from mere API hardening.
2. **Host replay.** The existing-claim branch constructs success from the requested host/ID plus the stored pane. Verify a conflicting same-ID request is rejected, and missing receipt recovery does not claim a launch that never dispatched. The brief permitted replay of a matching saved receipt; identify any material change in this contract.
3. **Lifecycle and locking.** `ownership` checks started before ended; normal terminal jobs have both. Trace consequences rather than treating an enum label alone as data loss. Verify uncertainty versus wrapper start in both orders, with all writes and readers considered.
4. **Regression strength.** The new fake-Herdr tests copy fabricated running/terminal JSON before returning a dispatch error. The error-first test retries host launch, not a delayed wrapper. Assess whether these tests actually prove the brief's deterministic two-order wrapper publication contract, and identify missing regressions precisely.
5. **Scope and documentation.** Original ownership covered nine job/runtime/work/test paths. The delivery also changes `src/main.rs` (one borrowing call-site adjustment), `README.md` (two lines), and `docs/design.md` (one line). Assess whether those changes are necessary and correct. In particular, distinguish refusing new side effects from claiming an already-started job has no process/log/pane.

These are lead observations to verify, not instructions to reproduce a predetermined verdict. Read all twelve changed paths and enough owning context to assess them.

## Evidence and check budget
Writer reports at this exact head: jobs CLI 12, lifecycle CLI 6, launch_retry CLI 34, review closed-stdout 1, format/clippy and full Rust gates passing. Focused acceptance reportedly ran as one chain, exit 0. Lead verified exact branch SHA, full diff and `git diff --check`.
Read-only review. Do not rerun tests/builds or install tools. If a concrete finding needs execution, propose the minimal deterministic regression and expected red result. Preserve original process-group/timeout and interrupted admission negative controls.
Owned/generated paths: none. No source/docs/config edits, commits, pushes, PR mutations, hosted CI, deployment or fleet cleanup.

## Publication breach
Lead verified OPEN draft PR #155 at the exact head. The writer published despite explicit local-commit-only authority. Do not mutate it. Keep this scope breach separate from code correctness findings.

## Reply
Return pass/block, severity-ordered findings with exact file/line references and causal scenarios, coverage gaps, and the smallest bounded repair if required. Name reviewed base/head and commands actually run. Complete the seeded review unit and use native done; do not create another task list.
