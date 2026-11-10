# T5: make the CLI suite reliable at base, and land the test-fix stack

**Outcome:** branch `test/detail-route-kilobytes` holds four commits on `a479b9337e57a3bfa38a22fad6a2dcfe01eb1d34`. Each commit passes the configured commit hook with no bypass.
1. A fix for `launch_retry::remote_preparation_release_requires_correlated_ack`.
2. A fix for `review_fixes::activity_uses_transcript_not_pi_footer_and_sanitizes_screen_and_jobs`.
3. T1's approved details test diff.
4. The approved `wait-discovery` edit.

**Why.** Every commit hook on 10 November 2026 failed on a test the commit did not touch. That blocks all landing. The suite must pass for every writer.

**Failure 1: `launch_retry::remote_preparation_release_requires_correlated_ack`** (`tests/cli/launch_retry.rs:250–312`).
- **When it runs alone,** it fails every time on netcup, in both the sidebar and U4 worktrees (base plus unrelated changes), in about 0.4 s. The failure is at `:310`, `log.matches("host prepared ").count()`: left 0, right 1. `ssh.log` holds only the `host observation` call.
- **In the full suite,** it passed in the U4 hook and in a `--no-fail-fast` run at `67dc4b7`.
- **In the full suite on momokaya-2** (T1 hook), it failed with 2 calls, where the test expects 1.

So the result depends on test order, timing, or the host. Find which case fails and why. Likely areas: the peer observation that `super::place::peer_observer` and `rig.ingest_peers()` set up before `place --host netcup`, and the `Rig::short` state. Fix the cause in the test fixture, or in product code if the product is wrong. If the product is wrong, report that first.

**Failure 2: `review_fixes::activity_uses_transcript_not_pi_footer_and_sanitizes_screen_and_jobs`** (`tests/cli/review_fixes.rs:66–76`).
- On momokaya-2 in the full suite, it returned `{deadline:false, error:"Text file busy (os error 26)"}` where it expected `"tool active"`.
- That is ETXTBSY when the herdr stub that `rig.script` (`tests/cli.rs:897–903`) rewrote is executed.
- Linux returns ETXTBSY if any process still holds a write fd to the file. Another test thread may fork while `fs::write` holds the temporary file open. The forked child keeps that fd until its own exec, and `rename` keeps the same inode.

Fix it in the rig for every caller of `script`. Do not retry the exec, and do not change product code to retry. Report the mechanism you chose and why it removes the race.

**Commits 3 and 4:** apply `/tmp/t1.patch` (details test) and `/tmp/t1-review.patch` (wait-discovery) on netcup exactly. The lead reviewed both. Commit messages:
- `test(observation): prove the over-frame detail route with kilobyte payloads`
- `test(review): give wait-discovery room for startup within its no-SSH bound`

**Owned paths:** `tests/cli.rs` (rig helpers only), `tests/cli/launch_retry.rs`, `tests/cli/place.rs` (fixture helpers only), `tests/cli/review_fixes.rs`, `src/observation/tests/details.rs` and `tests/cli/review.rs` (through the patches only). For product code, ask the lead before you edit.

**Checks (only these):**
- `cargo test --test cli launch_retry` 5 times in a row alone.
- `cargo test --test cli review_fixes` 5 times in a row.
- `cargo clippy --all-targets -- -D warnings` and `cargo fmt --check`.
- The configured hook on each commit.

`web/public` must exist. Copy it from `/var/lib/workloads/home/dev/dev/dogsled/.worktrees/fix-sidebar-terminal-loss/web/public`. Do not commit it. If a hook fails on another test you did not touch, stop and report the test name and the log lines. Do not retry the hook to get a pass.

**Long hooks:** your foreground tool stops at 600 s. Run each commit as a Dogsled job (`dogsled run --cwd <worktree> -- git commit -m ...`) and read its output.

**Publication:** push `test/detail-route-kilobytes`. Do not open a PR, merge, release, or install.

**Base:** `a479b9337e57a3bfa38a22fad6a2dcfe01eb1d34`. Check `git rev-parse HEAD` first.

**Reply:**
- branch, the four SHAs, and the changed paths;
- the root cause of each failure, with file:line;
- each check with its result;
- each hook's test summary line.
