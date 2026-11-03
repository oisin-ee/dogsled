Task: C5 round 2 (PR #222). Your branch is fix/admission-cancellation-r2. Run "git fetch origin && git merge --ff-only origin/fix/admission-cancellation" first, then CHECK that git merge-base --is-ancestor b33168a HEAD succeeds. You are a standalone writer: create and start one self-assigned todo leaf before running shell commands. Copy any existing .worktrees/*/web/public into ./web/public before committing. Commit in the background (this PATH is for the hook only; run your own cargo tests with your NORMAL PATH):
nohup sh -c 'PATH="$HOME/.cargo/bin:$PWD/.mise/shims:$HOME/.local/share/mise/shims:$PATH" mise x -- git commit -m "<msg>" > /tmp/c5r2.log 2>&1; echo EXIT=$? >> /tmp/c5r2.log' >/dev/null 2>&1 &
Then poll the log. If launch_retry::prepare_and_launch_wait_for_repository_lifecycle_lock flakes, retry up to 3 times.

The review of 0e03bb5 BLOCKED with 1 P1 and 3 P2s. Line numbers are from 0e03bb5, so re-locate them by symbol in b33168a. If one is already fixed at b33168a, prove it with a test and say so. Each fix gets a test that is red before it.
(P1) work.rs ~565-566: an unvalidated placement disposition can release ANOTHER request's claim. Copying request B's valid disposition into A's journal passes "expectation" validation (which checks the grant, not the disposition), and cancellation then sends B's intent to C3, bypassing C4's validate_saved. Fix: before cancellation sends ANY disposition intent, validate the saved disposition and binding against the request (request id, host, claim and binding) through the same validate_saved / correlated checks C4 uses. A mismatch refuses, sends nothing, and keeps a visible hold. Test: B's disposition in A's journal, then cancel A, gives no C3 call, B's claim untouched and A held with a reason.
(P2a) work.rs ~572-581: cancellation competes with immutable host dispositions. After a lost launch reply, the Cancelled intent conflicts with an existing transfer fence, so release repair is impossible; and if Cancelled wins before native-refusal settlement, unit_receipt later tries UnitRefused, conflicts, and withholds valid refusal evidence. Fix: cancellation FIRST reads the host's existing disposition through the correlated lookup. If one exists (Transfer, UnitRefused, AgentRefused or another Prohibit), cancellation ADOPTS it: it records the cancel intent at the origin only and sends no competing basis. Release follows the existing disposition's path, and refusal settlement still uses its own evidence. Only when no disposition exists does it send Cancelled. In a host-side race the first immutable disposition wins, and the loser adopts it rather than erroring. Tests: a lost launch reply with a transfer fence, then cancel, is repairable; native refusal racing cancel in both orders gives the refusal evidence settled once and the claim released once.
(P2b) work.rs ~641-647: a completed placement can still be cancelled. place --resume can complete the journal while the ticket stays pending; cancel checks only the ticket, accepts, and fences the saved result. Fix: decide "too late" from the ADMISSION JOURNAL (completed or result saved) under the admission lock, not from the ticket state. Test: resume completes, the ticket is still pending, then cancel returns too-late, and the saved result stays readable.
(P2c) work.rs ~689-691: retention removes cancelled Job tickets but leaves the cancellation records; repair then calls project_cancellation, which errors on the missing ticket and aborts EVERY admission tick. Fix:
(i) a missing ticket for a COMPLETED cancellation is a no-op, and an incomplete one is a per-request visible hold;
(ii) one bad request never aborts the tick for unrelated requests (isolate errors per request and report them);
(iii) retention drops the cancellation record together with its ticket, in the same owner, only when the cancellation is terminal.
Test: expire a cancelled job ticket, then the next tick admits an unrelated request.
Owned files: src/work.rs, src/unit_request.rs, src/runtime.rs, src/mcp.rs, src/work_tests.rs, tests/cli/unit_requests.rs, tests/cli/place.rs, tests/cli/mcp.rs, tests/cli/jobs.rs. PRE-AUTHORIZED: additive use of the C3 lookup and dispose, plus src/prepare.rs, src/prepare/disposition.rs and src/cleanup.rs where needed for P2a or P2c, without changing existing variant behaviour; list them in the commit. Fixtures use short /tmp roots and git init -b main; serve --once tests need rig.config zero thresholds plus a seeded snapshot.
Acceptance:
- cargo test --test cli unit_requests::
- cargo test --test cli place::
- cargo test --test cli mcp::
- cargo test --test cli jobs::
- cargo test work_tests
Rules:
- Make the first edit within 3 tool calls.
- Commit through the moon hook; never use --no-verify.
- Push with git push origin HEAD:fix/admission-cancellation (a fast-forward) and git push -u origin HEAD. Check both with ls-remote. Pushing is authorized.
- Push within 30 minutes.
- For anything else outside scope, send a one-line message and CONTINUE; don't end your turn blocked.
- Your final reply is one line with the SHA.