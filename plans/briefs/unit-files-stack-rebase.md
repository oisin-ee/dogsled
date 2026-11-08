# U9: Rebase the Files stack onto main and keep preservation unbounded

## Outcome
- The ten commits `3fdd5cf..2ad1814` (U5 to U8) sit on `1f5498e` (#302 rebased onto `origin/main` at `03d8d08`).
- Every commit compiles on its own.
- Preservation reads Git output without a size cap again.

## Cause
- `origin/main` now has #304:
  - It adds `Shell::run_input_bytes_bounded(program, args, input, timeout)`.
  - `preservation::raw_input` reads every snapshot blob in one `git cat-file --batch`.
- U6a (`a3d532a`) added a `limit` to `run_bytes_bounded`/`execute_bytes`, and passed `crate::shell::OUTPUT_LIMIT` (detail codec wire limit + 64, about 32 MiB) from `preservation::raw`.
  - Since U6a, a preserved file over 32 MiB fails close with `Error::OutputLimit`. Before U6a there was no cap.
  - With #304's batch read, the same cap would apply to the sum of all snapshot blobs.
  - Preservation's contract is to read everything it preserves; it has no size policy. The limit belongs to each caller, so preservation must state "no cap" explicitly.
- `git rebase` stops at `a3d532a` with a conflict in `src/preservation.rs` `raw_input`.

## Edits
1. **Refs:** in your worktree, create local branches at the old heads:
   - `stack/files-next` = `08acf20`
   - `stack/files-part` = `c0470b9`
   - `stack/files-tab` = `09e56ab`
   - `stack/accordion` = `f47c990`
   - `stack/open-once` = `2ad1814`

   Then run, from `stack/open-once`:
   `git rebase --update-refs --onto 1f5498e 0a20ca3 --exec 'cargo check --all-targets'`
2. **Fix `a3d532a` inside that commit:**
   - `run_input_bytes_bounded` gains `limit: usize`, in the `Shell` trait, `System` (pass it to `execute_bytes`), and every implementation and caller: `src/preservation.rs` tests, `src/loss_tests.rs`, `src/observation/tests/context_fixture.rs`, and `src/shell.rs` tests.
   - In `src/preservation.rs`, add `const READ_LIMIT: usize = usize::MAX;`. Its doc comment says that preservation must read every byte it preserves and has no size policy.
   - Pass `READ_LIMIT` from `raw_input` for both branches. Resolve the conflict to #304's `words`/`budget`/`match input` shape.
3. **Other conflicts:** resolve each inside the commit that causes it. Keep both sides' intent, and check that main's lines survive. `git diff 03d8d08 <new tip> -- <file>` must show only the stack's own changes.
4. **Test, in the `a3d532a` rewrite:** a `src/preservation.rs` test with a recording `Shell` that asserts `raw` and `raw_input` pass `READ_LIMIT`, not `OUTPUT_LIMIT`.

## Paths
- **Owns:** `src/shell.rs`, `src/preservation.rs`, `src/loss_tests.rs`, `src/observation/tests/context_fixture.rs`, and any file a rebased commit conflicts in.
- **Reads:** `origin/main` at `03d8d08`, and `plans/briefs/unit-files-capture.md`.

## Acceptance
- `--exec cargo check --all-targets` passes at every commit.
- At the new tip, these pass: `cargo test --bin dogsled preservation`, `shell`, `git_status`, `loss`, and `details`, plus `cargo test -p web` and `cargo clippy --all-targets -- -D warnings`.
- Report the five new branch SHAs. Do not push.

## Checks
- Run `cargo fmt --all --check` at the tip.
- The lead builds `web/public` in your worktree before you start. Wait for `BUNDLE_EXIT=0` in `/tmp/u9-bundle.log`, and don't run web-build yourself.
- A rebase creates no commits through the hook, so none are needed.

## Base
Branch `rebase/files-stack` at `2ad1814`; the rebase target is `1f5498e` on `origin/integrate/files-refresh`. This brief is complete; it depends on no plan file.
