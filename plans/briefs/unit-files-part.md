# U6b: Files detail part, revision and route

## Outcome
An open agent's checkout status becomes a third detail part, beside the conversation and the screen.
- It has its own revision in `AgentEvent.revisions`.
- It is served at `GET /api/agents/{host}/{pane}/files`, for local and remote agents.
- A slow Git read never delays the conversation or the screen.

## Cause
- U6a added `crate::git_status::read` and `portal_types::files::FilesState`. Nothing captures or serves them yet.
- The detail worker (`src/observation/details.rs` `Worker::start`) runs every read in one loop. If Git ran in that loop, a slow repository would block the conversation and screen parts.

## Edits
1. **`portal-types/src/agents.rs`:** `DetailRevisions` gains `files: u64`, with `#[serde(default)]` so older peers still parse.
2. **`src/observation/details.rs`:**
   - `Detail` gains `files: FilesState`. It is `Pending` in `Detail::pending`, and `Unavailable(reason)` in `Detail::unavailable`.
   - `Reference` gains a `files: ReadState`: `Empty` for a current state with zero entries.
   - `Payload::new` computes `parts.files` the same way as `conversation` and `screen`.
   - **Files thread:** `Worker::start` starts a second thread. It reads `git_status::read(&crate::shell::System::default(), agent.cwd.as_deref(), timeout)` every `DETAIL_TICK`, using the same `stopped` flag, and stores the newest result in a shared slot.
     - The slot is tagged with the session and the checkout root it was read for.
     - The thread wakes the main loop through `wake` with a new `Tick::Files`.
   - **Main loop:** it copies the slot into `detail.files` and never waits for Git.
   - **Fencing:** a result whose session no longer matches is discarded.
   - **Stale:** after a `Current` for the same root, a failed read becomes `Stale { files, error }`. A different root never inherits a stale snapshot.
   - **Stop:** retiring the worker stops and joins both threads. `Registry::retire` keeps its current non-blocking retire.
3. **`src/web/agents.rs`:**
   - Add `PeerOperation::Files` and the route `/api/agents/{host}/{pane}/files`.
     - **Local:** it waits until `files` is not `Pending`, then answers 200 with the `FilesState` JSON.
     - **Remote:** it goes through `peer_response`.
   - Generalise `detail_response` so one function serves all three parts. Its `select` returns a response, so `Content<T>` parts and `FilesState` share one subscription, deadline and stamping loop. Do not copy the loop.
   - `peer_read` handles `Files`. `peer_follow` needs no change, because `event_reply` already carries `publication.parts`.
   - **Older peer:** one that rejects `files` as an unknown operation answers as a peer error. The route maps it to 502 "host {host}: files unsupported". No fallback.

## Paths
- **Owns:** `portal-types/src/agents.rs` (`DetailRevisions` only), `src/observation/details.rs`, `src/observation/details_fixture.rs`, `src/observation/tests/details.rs`, `src/web/agents.rs`.
- **Reads:** `src/git_status.rs`, `portal-types/src/files.rs`, `src/web/peer_stream.rs`, `src/shell.rs`.

## Acceptance
Tests in `src/observation/tests/details.rs`, with a real temporary repository as the agent's `cwd`:
- Editing a file bumps `parts.files` only. `conversation` and `screen` keep their revisions.
- An unchanged tree publishes no new revision.
- A Git read held for 3 s by a fixture shell still lets a transcript change publish within 1 s.
- A session change discards the old result, and the new session starts `Pending`.

In `src/web/agents.rs` tests:
- The local files route answers the current state.
- The remote route sends `{"operation":"files"}` over the peer command and relays the reply.
- An event line for a files change carries `revisions.files`.

Commands, all of which must pass:
- `cargo test --bin dogsled observation::tests::details`
- `cargo test --bin dogsled web::agents`
- `cargo test -p portal-types`

## Checks
- Run `cargo fmt --all --check` and `cargo clippy --all-targets -- -D warnings`.
- The lead builds `web/public` in your worktree and runs web-lint. Don't run web-build. Wait for `BUNDLE_EXIT=0` in the log the lead names before you compile the server crate.
- Commit through the pre-commit hook, without bypassing it.

## Base
The revision that contains U6a and U5, named in the spawn. This brief is complete; it depends on no plan file.
