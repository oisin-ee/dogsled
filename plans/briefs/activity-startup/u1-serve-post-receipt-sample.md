# U1: serve verifies a producer registration against a sample taken after receipt

**Outcome:** a registration for a pane or Pi session file that is newer than the last `herdr agent list` sample succeeds in one authority apply. `agent_missing` and `session_unverified` come only from a sample taken after the registration arrived.

**Cause.** `src/observation/live_activity.rs` `Authority::register` (line ~316) resolves the pane through `binding()` (line ~287). That reads `self.bindings`, the last sample, and never samples after receipt. `binding()` also rejects a cached `evidence: None` with `session_unverified` before `register()` re-checks the header at line ~330. Pi creates the session file at the first message, and Herdr learns the pane at `session_start`. So the first registration always meets a stale sample and returns 404 or 503. Pending commands are drained in `refresh_at` (line ~196), which has no shell. `service.rs` `publish()` calls `activity.refresh(&collection.input)` (line ~354). Full plan: `plans/activity-startup-registration.md`.

**Edits**
1. `live_activity.rs`: `refresh`/`refresh_at` take `shell: &dyn crate::shell::Shell`.
   - Drain the pending commands, then observe the input terminals as today.
   - If any pending `Register` cannot be verified from the cached state, call `self.sample(shell, now_ms)` once before applying commands. "Cannot be verified" means any of these: global sample older than `NATIVE_TTL_MS`, pane missing, `session` is `None`, or `evidence` is not `Some(body.evidence)`.
   - Take at most one forced sample per `refresh_at`. `Publish` commands never force a sample.
   - Make `sample` use the passed `now_ms`, so tests are deterministic.
2. `live_activity.rs`: update the comments at lines ~181–182 and ~324–326. Unverifiable registrations now force one post-receipt binding sample. Publishes, heartbeats, and viewers still never do.
3. `live_activity.rs`: log each registration refusal with `eprintln!("dogsled activity: register {pane} refused: {code:?}")`.
   - Log once per change of `(pane, code)`. Remember the last code per pane, clear it on success, and drop it when the pane leaves `bindings`.
   - Never log evidence paths or request bodies.
4. `service.rs`: pass `node.shell` to `activity.refresh`.
5. `src/observation/tests/live_activity.rs`: add tests using `crate::shell::Fake` (see the existing uses at lines ~17, ~806, ~872). Update the `Fixture` call site of `refresh_at`. The new tests cover:
   - (a) A pane absent from the last sample but listed by Herdr registers in one `refresh_at`, with exactly one `herdr agent list` call.
   - (b) A session file created after the last sample registers.
   - (c) A pane still absent after the forced sample gives `AgentMissing`.
   - (d) `Publish` traffic and `views()` add no `herdr agent list` calls.
   - (e) Three pending registrations force only one sample.
   - Existing CAS, no-steal, and replay tests keep passing unchanged.

**Owned paths:** `src/observation/live_activity.rs`, `src/observation/tests/live_activity.rs`, `src/service.rs`. Read anything else. Do not touch `pi/`, the wire types in `portal-types`, or the HTTP handlers. HTTP handlers must not do native reads.

**Acceptance:** `cargo test --bin dogsled live_activity` passes, including tests (a)–(e).

**Checks (only these):** `moon run dogsled:web-build` once, then `cargo test --bin dogsled live_activity`, `cargo test --bin dogsled web::activity`, `cargo clippy --all-targets -- -D warnings`, and `cargo fmt --check`. Commit hooks run as configured. Do not bypass them.

**Publication:** commit with `fix(activity): verify producer registration against a post-receipt sample`. Push your branch. Do not open a PR, merge, release, or install.

**Base:** `a479b9337e57a3bfa38a22fad6a2dcfe01eb1d34`.

**Reply:** branch, head SHA, changed paths, the commands with their results, and anything unverified. If the design does not fit the code, stop and report the conflict instead of widening scope.
