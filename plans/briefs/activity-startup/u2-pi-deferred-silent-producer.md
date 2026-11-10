# U2: the Pi producer registers once its session is verifiable and never writes to the console

**Outcome:** a new Pi session sends no activity request before its session file exists. It registers at the first boundary event after the file exists. It never writes to `console` or stderr. An actionable terminal failure gives one `ctx.ui.notify` warning.

**Cause.**
- `pi/activity.ts` `ActivityPublisher`'s constructor (line ~200) queues the registration POST at `session_start`.
- Pi v1.5.0 writes the session file only at the first user or assistant message (`SessionManager._persist`). Serve verifies the producer by reading that file's header, so a new session can never register until the first prompt.
- `diagnostic()` (lines ~437–441) calls `console.error` on start, retry, acknowledgement, stop, and dispose. In TUI mode Pi does not redirect stderr, so these lines paint over the TUI.
- Full plan: `plans/activity-startup-registration.md`.

**Edits**
1. `pi/activity.ts` `ActivityPublisher`:
   - Add constructor parameters `exists: (path: string) => boolean` (default `fs.existsSync`) and `report: (message: string) => void`.
   - Keep reducing events from construction.
   - Do not send or schedule the registration POST until `exists(evidence.session_file)` is true. Check it in the constructor and in a new public method `evidenceReady()`. Do not poll it on a timer.
   - The heartbeat must not send requests before registration is armed.
2. `registerActivity`:
   - Call `runtime?.evidenceReady()` on `message_start` and `message_end` (any role), and on `turn_end` and `agent_settled`.
   - Never call it on `message_update` deltas.
   - Pass `report` as a function that calls `ctx.ui.notify(message, "warning")`. Use the `ctx` from `session_start`, and wrap the call in try/catch.
3. Delete `diagnostic()` and every `console.error`.
   - `stop()` calls `report` once per publisher with `Dogsled live activity stopped: <code> (HTTP <status>)`.
   - `dispose()` and every retry and backoff path stay silent.
   - Keep the existing bounded backoff and exact-request replay for `agent_missing`, `session_unverified`, `upstream_unavailable`, transport, and 5xx outcomes. Keep their handling unchanged apart from removing the console output.
4. `pi/index.ts`: change it only if the `registerActivity` signature must change.
5. `pi/activity.test.ts`:
   - Update the `rig` so that a missing file can be simulated.
   - Make `rig` assert that `console.error` and `process.stderr.write` are never called in any test.
   - Add tests:
     - (a) A missing file means no request across 30 s of timers. After the file appears and `evidenceReady()` is called, the publisher sends `POST` then `PUT`.
     - (b) An existing file registers at construction, which covers resume and reload.
     - (c) Each terminal code (`unauthorized`, `unsupported_version`, `invalid_request`, `sequence_conflict`, and an invalid acknowledgement) calls `report` exactly once.
     - (d) The `agent_missing`, `session_unverified`, and `upstream_unavailable` retries call `report` zero times and still recover.
   - Replace the `diagnostics` assertions in the existing tests accordingly.

**Owned paths:** `pi/activity.ts`, `pi/activity.test.ts`, `pi/index.ts`. Read anything else. Do not touch `src/`, the wire contract, or other extensions.

**Acceptance:** `node --test pi/activity.test.ts` passes, including tests (a)–(d), and no test observes console or stderr output.

**Checks (only these):** `node --test pi/activity.test.ts`, `node --test pi/index.test.ts`, and `moon run dogsled:typecheck`. Commit hooks run as configured. Do not bypass them.

**Publication:** commit with `fix(pi): register live activity once the session is verifiable and stop console output`. Push your branch. Do not open a PR, merge, release, or install.

**Base:** `a479b9337e57a3bfa38a22fad6a2dcfe01eb1d34`.

**Reply:** branch, head SHA, changed paths, the commands with their results, and anything unverified. If the design does not fit the code, stop and report the conflict instead of widening scope.
