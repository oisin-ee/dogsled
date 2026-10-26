# J1 repair: exact claims and proven replay

## Outcome and owning cause
A wrapper runs only its requested job's valid pending claim. A host never turns allocation evidence into a successful dispatch receipt.
`runtime::exec` loses the requested ID when it passes a saved Job onward. `jobs::pending_claim` validates only some fields. `work::launch` reconstructs success from any existing claim without a matching receipt.

## Revision and scope
Repository: `/Users/oisin/dev/dogsled`.
Reuse `fix/lifecycle-job-claims` WITHOUT a base argument. Its retained head is `1a965411b2430e4f3dc9abf1c305e120bd1ac778`; the original writer has closed.
Preserve its commits. Do not reset, recreate, or rebase the branch.
Prerequisite: independent review `u-d5011b8e-2279-4357-b103-19df3416b805-58` returned BLOCK at that exact SHA. Lead verified each owning path.
Owned: `src/jobs.rs`, `src/runtime.rs`, `src/main.rs` (exec caller only), `src/work.rs`, `src/work_tests.rs`, `tests/cli.rs` (job fixture helpers only), `tests/cli/jobs.rs`, `tests/cli/launch_retry.rs`, `tests/cli/lifecycle.rs`, `tests/cli/review.rs`, `README.md` and `docs/design.md` (J1 paragraphs only).
Generated: none. Read applicable local/code-standard rules, `src/ledger.rs`, `src/shell.rs`, and the original J1 brief.
The C7 repair owns worktree/close/done tests; do not edit those paths or main checkout.

## Required repair
1. Keep the requested ID through the execution/start boundary. A record's embedded ID must match its filename/request; neither lookup nor lock selection may switch to another ID. Reject a contradictory identity before any start publication or command/log/pane effect.
2. Under the lifetime execution lock and short start lock, validate the complete pending claim. Require its actual allocated pane, correct pending state, no started/ended/exit/signal/reason evidence, nonempty command, and valid recorded execution identity. Compare execution-affecting inputs, including command/options and the immutable fields passed from the initial record, rather than silently adopting changed cwd/host/owner/pane/options during reload. Do not add retries or silently normalize invalid records.
3. Verify intent-to-claim publication still belongs to the exact same request. Check the immutable fields and allowed transition instead of accepting any record labelled `claiming`. Preserve the non-executable intent before allocation.
4. A claim without matching saved success evidence is not a completed dispatch. Refuse missing-receipt recovery without allocating, dispatching, overwriting the claim, saving a success receipt, or releasing preparation ownership. Keep uncertainty and existing wrapper evidence intact. A matching saved success receipt may replay its result only after validating correlation to the request and recorded job; conflicting same-ID command/options/cwd/host/owner/pane evidence must refuse. Do not add an acknowledgement protocol or reconstruct success from timestamps alone.
5. Preserve the existing short-lock start/uncertainty ordering, lifetime execution lock, and actual process-group/timeout/final-record behavior. No short lock may span pane submission or command lifetime; no execution lock inside a launch/store lock. The terminal-as-Started enum label is not a separate demonstrated defect and needs no unrelated redesign.
6. Correct the J1 README/design comments: refusal creates no NEW process/log/pane effects; already-started jobs and valid pending claims can have existing resources. Keep the necessary `main.rs` borrowing adjustment rather than suppressing Clippy or reverting to an invalid signature.

## Required regressions
Add faithful failing tests on `1a96541`, then fix the owning production logic:
- A.json embeds B, both commands match, and B is valid with distinct cwd/options/pane. Executing A must refuse; both records remain unchanged and no process/log/pane operation occurs.
- Pending records with empty pane, terminal exit/signal/reason, mismatched identity, or conflicting immutable claim fields refuse without mutation. Remove the new expectation that an empty-pane pending claim executes. Preserve legitimate no-current-HERDR-pane behavior using a valid claim if that existing check remains applicable.
- Interrupted launch after claim publication but before `pane run`: retry fails closed, preserves the claim, produces no success receipt, and neither allocates nor dispatches. Cover conflicting same-ID requests with and without a saved receipt, plus valid matching receipt replay without side effects.
- Use real wrapper execution with deterministic barriers for both orders: wrapper publishes started/terminal before dispatch error, and dispatch error records uncertainty before a delayed wrapper starts. In the first order preserve real running/terminal output; in the second refuse execution and leave the command marker absent. Do not substitute copied JSON for wrapper publication.
- Reuse the FIFO/process synchronization patterns in the existing CLI tests. Bound subprocesses/waits and clean up everything the test starts. No sleeps, production test-only flags, new dependencies, or detached test processes.
- Keep `interrupted_job_admission_preserves_running_record_without_relaunch` unchanged as the negative control. Keep real timeout, process-group, closed-stdout, and final-record-before-close checks.

## Acceptance and check budget
In the linked worktree run:
`cargo test --test cli jobs:: && cargo test --test cli lifecycle:: && cargo test --test cli launch_retry:: && cargo test --test cli review::closed_wrapper_stdout_preserves_full_log_and_child_exit && cargo test --bin dogsled work_tests::`
Allow scoped formatting and required pre-commit hooks (`mise run check:rust`). Never bypass hooks; do not repeat full suites outside required hooks.
Report red-before and green-after commands, exit status, and exact final SHA. Preserve Cargo's status when output is piped.

## Authority and handoff
Allow scoped LOCAL commits only on the retained branch. NO push, PR creation/update, merge, release, deployment, live cleanup, dependency changes, or unrelated edits. Existing draft PR #155 was unauthorized: do not update its remote branch or PR.
J2 request age and C8 retention remain separate and blocked. Do not add timestamps, expiry, new retries, or a new provider protocol. If exact correlation cannot be implemented within these paths, stop with the source blocker rather than inventing success.
Return branch, SHA, changed paths, causal repair, regression results, and remaining gaps. Use the seeded unit and native done; do not create another task list.
