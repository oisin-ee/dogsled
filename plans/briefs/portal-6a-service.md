# Brief: `dogsled web` as a launchd service, with clean SIGTERM shutdown (web portal step 6a)

**Outcome.**
- Release archives ship `plugin/services/dev.dogsled.web.plist`, which runs `dogsled web` at login and keeps it alive.
- `dogsled web` exits within 2 s of SIGTERM or SIGINT, even while browsers hold SSE streams open.

**Cause / design.**
- The plist has the same shape as `plugin/services/dev.dogsled.plist`:
  - Label `dev.dogsled.web`.
  - ProgramArguments `@HOME@/.local/bin/mise x -- dogsled web`.
  - The same RunAtLoad, KeepAlive, ThrottleInterval and EnvironmentVariables as the existing plist.
  - Logs at `@HOME@/.local/state/dogsled/web.stdout.log` and `web.stderr.log`.
- **The shutdown bug, seen by the lead:** two `dogsled web --port 7467` processes ignored SIGTERM and SIGINT. Only `kill -9` stopped them. launchd stops services with SIGTERM, so this blocks the service.
  - `src/web/mod.rs:146` uses `with_graceful_shutdown(shutdown())`, but axum waits for every open connection to finish. SSE streams never finish: the board `/api/stream`, `/api/agents/{pane}/events` and the decisions stream.
  - After serve returns, `run()` also awaits `reader`, a blocking thread.
  - The Herdr `Subscription` (`src/herdr_socket.rs`) reads with no timeout, so a blocking thread can hold the runtime open at drop.
  - Reproduce first, and confirm which of these hold the process. Then fix each cause where it lives:
    - Every long-lived stream and long-poll ends when shutdown begins. Use one shared shutdown signal in AppState, for example a `tokio_util::sync::CancellationToken` or a `watch` channel, and stop each stream with it.
    - Blocking threads observe the stop flag and return.
  - Do not add a forced exit or a sleep-then-exit timer.

**Edits.**
- New `plugin/services/dev.dogsled.web.plist`.
- `dist-workspace.toml`: add it to `include`.
- `src/web/` modules and `src/herdr_socket.rs`: the shutdown fix.
- `README.md`:
  - The archive line near 659 lists the new plist.
  - Add a short "Web portal" section: `dogsled web`, port 7465 (`[web] port`, `DOGSLED_WEB_PORT`), the token file, the plist, and the `dogsled interview` and `dogsled review` fallbacks.

**Owned paths:** plugin/services/dev.dogsled.web.plist, dist-workspace.toml, README.md, src/web/, src/herdr_socket.rs, Cargo.toml, Cargo.lock.

**Acceptance.**
1. `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` and `cargo test --bin dogsled web::` pass.
2. A test starts the server on port 0 and opens `/api/stream`. It triggers shutdown, and then the serve future completes within 2 s.
3. `plutil -lint plugin/services/dev.dogsled.web.plist` passes.
4. CI on the PR passes: rust on macOS and Ubuntu, and typescript.
5. Report the cause you confirmed and the evidence for it.
