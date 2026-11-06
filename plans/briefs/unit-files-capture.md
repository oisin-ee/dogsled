# U6a: Git status capture, wire types and By Stage projection

## Outcome
Serve can read one checkout's `git status` into exact typed entries, and both the server and the portal can group those entries by stage with one shared pure function. No route, worker or UI uses this yet. U6b adds those.

## Cause
- `src/shell.rs:238` collects output with `wait_with_output` and has no size limit, so a huge `git status` would be read into memory whole.
- `src/loss.rs::dirty_paths` drops status codes and can filter deletions, so it can't serve Files.

## Edits
1. **`src/shell.rs`.**
   - `System::execute_bytes` takes `limit: usize` for stdout. It reads stdout through a bounded reader. At `limit + 1` bytes it stops, kills the process group with `kill_group`, and returns a new `Error::OutputLimit { operation, limit }` (`src/error.rs`).
   - `Shell::run_bytes_bounded` gains the `limit` argument. `Fake` honours it too.
   - The text paths (`execute`, `run`, `run_bounded`, `run_input_bounded`) pass `OUTPUT_LIMIT`. Set it to the largest legitimate reader today, the SSH peer reply, whose body is at most `crate::observation::details::CODEC.wire_limit()` plus the `PeerReply` envelope, and give the reason in a doc comment.
   - Update every `run_bytes_bounded` caller (`src/preservation.rs`, `src/loss_tests.rs`, `src/observation/tests/context_fixture.rs`, `src/shell.rs` tests) with its own explicit limit.
2. **`portal-types/src/files.rs` (new), exported from `portal-types/src/lib.rs`.** Data and pure functions only, so it builds for wasm32.
   - `GitPath { text: String, bytes: Option<Vec<u8>> }`. `bytes` is set only when the name is not valid UTF-8; `text` is then the lossy form. Two distinct byte paths never compare equal.
   - `Entry { path: GitPath, change: Change }`. `Change` is one of:
     - `Ordinary { index: char, worktree: char }`;
     - `Renamed { index: char, worktree: char, from: GitPath }`, which covers both rename and copy;
     - `Conflict { code: [char; 2] }`, for all seven unmerged codes: `DD AU UD UA DU AA UU`;
     - `Untracked`.
   - `Head`: `Branch(String)`, `Detached { oid: String }` or `Unborn { branch: String }`.
   - `Files { root: String, head: Head, entries: Vec<Entry>, captured_ms: u64 }`.
   - `FilesState`: `Pending`, `Current(Files)`, `Stale { files: Files, error: String }`, `NotRepository`, `TooMany`, `Unavailable(String)`.
   - `by_stage(&[Entry]) -> Stages { conflicts, staged, unstaged, untracked, paths }`.
     - Conflicts are the conflict entries only.
     - Staged is non-conflict entries whose index code is not `.`; Unstaged is those whose worktree code is not `.`. One path can be in both.
     - Untracked is the untracked entries.
     - Each group is sorted by path. `paths` counts unique paths.
3. **`src/git_status.rs` (new), declared in `src/main.rs`.** Run Git only through `&dyn Shell`.
   - **Root:** `git --no-optional-locks -C <cwd> rev-parse --show-toplevel`. Map "not a git repository" to `NotRepository`.
   - **Status:** `git --no-optional-locks -C <root> status --porcelain=v2 --branch -z --untracked-files=normal`, with a 1 MiB limit. Map `Error::OutputLimit` to `TooMany`.
   - **Parse** `# branch.oid` and `# branch.head` headers and record types `1`, `2` (path, then a NUL, then the original path), `u` and `?`. Ignored records are never listed. An unknown record is a parse error, not a skip.
   - **Wrap the result:** `pub(crate) fn read(shell, cwd: Option<&str>, timeout) -> FilesState` returns `Current`. A missing cwd gives `Unavailable`. The caller (U6b) turns a failure after a good read into `Stale`.
   - Use the repository's existing Git-environment isolation for child Git processes. Read the `GitFixture` in `src/shell.rs` tests and the `loss` and `preservation` callers.

## Paths
- **Owns:** `src/shell.rs`, `src/error.rs`, `src/git_status.rs`, `src/main.rs` (module line only), `portal-types/src/files.rs`, `portal-types/src/lib.rs` (module line only), and the `run_bytes_bounded` call sites in `src/preservation.rs`, `src/loss_tests.rs` and `src/observation/tests/context_fixture.rs`.
- **Reads:** `src/loss.rs`, `src/observation/details.rs`, `src/web/agents.rs`.

## Acceptance
- **Real temporary repositories** in `src/git_status.rs` tests: clean; staged and unstaged on one path; delete; rename with source; each of the seven conflict codes; untracked file and untracked directory (one row); a non-UTF-8 name next to its lossy twin (two rows); unborn HEAD; detached HEAD; a linked worktree; not a repository; no cwd.
- **Limit:** a test status over the limit gives `TooMany`. A `src/shell.rs` test shows a process writing past the limit is killed and returns `Error::OutputLimit` before it exits on its own.
- **Projection:** `portal-types` tests cover `by_stage` group membership, order and the unique-path count.
- **Commands:** `cargo test --bin dogsled git_status`, `cargo test --bin dogsled shell`, `cargo test -p portal-types`, and `cargo test --bin dogsled preservation` all pass.

## Checks
- Run `cargo fmt --all --check` and `cargo clippy --all-targets -- -D warnings`.
- The lead builds `web/public` in your worktree and runs web-lint. Don't run web-build. Wait for `BUNDLE_EXIT=0` in the log the lead names before you compile the server crate.
- Commit through the pre-commit hook, without bypassing it.

## Base
`integrate/files-refresh` at the SHA in the spawn. This brief is complete; it depends on no plan file.
