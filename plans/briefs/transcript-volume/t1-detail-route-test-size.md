# T1: the over-frame detail test proves its route with kilobytes, not 17 MB

**Outcome:** `observation::tests::details::conversation_over_main_frame_limit_loads_only_through_detail_route` proves the same two properties as today, but with kilobyte payloads. The test passes in well under 1 s, even when the host is loaded. The fixed 3 s fixture deadlines and production behaviour stay unchanged.

**Cause (measured 10 November 2026):**
- The test writes a synthetic 17 MB transcript line (`src/observation/tests/details.rs:592`), because it must exceed `ipc::MAX_FRAME_BYTES` = 16 MB (`src/observation/ipc.rs:20`).
- One detail tick must parse that line in a debug build within `recv_timeout(3 s)` (`src/observation/details.rs:575`), and `Fixture::wait` also allows 3 s (`src/observation/details_fixture.rs:117`).
- Under the full parallel suite on netcup (load 28 on 16 cores), the tick timed out. This failed the precommit hook for an unrelated change. Run alone, the test passes in 3.58 s.

**The two properties the test must keep proving:**
1. The main observation frame never carries conversation content. It stays within a small delta of the frame without the conversation (lines 603–610).
2. A conversation larger than the main-frame cap loads in full through `/api/agents/local/{pane}/conversation` (lines 611–638).

**Edits (choose the smaller design and say which one you chose):**
- **(A) Inject the limits:** the details collector and publisher take their main-frame cap and conversation read limit as constructor parameters. Production passes today's constants (`MAX_FRAME_BYTES`, `CONVERSATION_BYTES`, `details::CODEC`), and the test fixture passes kilobyte values. Do not use `cfg(test)` globals or environment variables.
- **(B) Split the properties:**
  - Test 1 keeps the end-to-end fixture with a kilobyte conversation, and asserts property 1 plus a full route round trip.
  - A separate test asserts the limit relation `details::CODEC.wire_limit() > ipc::MAX_FRAME_BYTES`, and that a payload larger than `MAX_FRAME_BYTES` survives `CODEC.encode` / `Codec::decode` and the route's body limit. There is already a codec test near line 721 to build on. This test must not run a service tick or wait on a deadline.
- In either design, every assertion in the current test must still be checked by some test. Do not raise, remove or loosen any deadline.

**Owned paths:** `src/observation/tests/details.rs` and `src/observation/details_fixture.rs`. For design (A) only, also `src/observation/details.rs`, `src/observation.rs` and `src/observation/ipc.rs`. Change only the constructor parameter plumbing; behaviour stays the same. Read everything else.

**Acceptance:** `cargo test --bin dogsled observation::tests::details` passes. Report the time of the changed test or tests from the test output.

**Checks (only these):** the filter above, `cargo test --bin dogsled observation`, `cargo clippy --all-targets -- -D warnings`, and `cargo fmt --check`. `web/public` must exist. If it does not, copy it from the lead's prepared worktree when the lead names one; otherwise ask the lead. Do not build it. Commit hooks run as configured. Do not bypass them.

**Publication:** commit with `test(observation): prove the over-frame detail route with kilobyte payloads`. Push your branch. Do not open a PR, merge, release or install.

**Base:** `a479b9337e57a3bfa38a22fad6a2dcfe01eb1d34`. Run `git rev-parse HEAD` first. If it differs, stop and report.

**Reply:** branch, head SHA, the design you chose, the paths you changed, each command with its result, and which test now covers each original assertion.
