# Brief: find Herdr's socket without HERDR_SOCKET_PATH, then release 0.25.3

**Outcome.** `dogsled web`, started by launchd outside any Herdr pane, lists agents, reads conversations, sends messages and keys. Release it as 0.25.3.

**Cause, confirmed by the lead on the installed 0.25.2 service.**
- The service answers `GET /api/agents` with `{"error":"herdr socket is not available"}`.
- `src/web/mod.rs:185` builds the client with `crate::herdr_socket::Client::from_env().ok()`.
- `src/herdr_socket.rs:28` `from_env()` reads only `HERDR_SOCKET_PATH`. Herdr sets that variable inside panes and plugin commands, but launchd does not. So every agent route fails under the service.
- `herdr status server --json` reports the socket path in a `"socket"` field, for example `"/Users/oisin/.config/herdr/herdr.sock"`.
  - It works with no Herdr variables set, and also when the server is not running: `{"status":"not_running",…,"socket":"…/sessions/nosuch/herdr.sock"}`.
  - It respects Herdr's own session rules, so do not hard-code `~/.config/herdr`.

**Edits.**
1. **`src/herdr_socket.rs`:** add `Client::discover()`.
   - Use `HERDR_SOCKET_PATH` when it is set and non-empty.
   - Otherwise run `herdr status server --json` through the existing command runner, `crate::shell` (the same way other code runs `herdr`; keep its 30 s budget), and use the `"socket"` string.
   - If neither gives a path, return an error naming both sources.
   - Keep `from_env()` for the board and sidebar, which run inside Herdr and must keep their current behavior.
2. **`src/web/mod.rs`:** use `discover()` for the portal's client.
   - When discovery fails at startup, log one line.
   - Keep the existing "herdr socket is not available" response.
   - The socket file may not exist yet at login: connections already fail per request and then work once Herdr runs, so do not resolve the path again per request.
3. **Tests:**
   - Env set: uses the env path.
   - Env unset: uses the path from a fake `herdr status server --json`. Follow how existing tests fake `herdr` commands.
   - Neither: returns the error.
4. **Release commit `chore(release): 0.25.3`:**
   - Change the same nine files as `git show ffc76ad`, replacing 0.25.2 with 0.25.3.
   - CHANGELOG `## 0.25.3 - 2026-10-27`: "`dogsled web` finds Herdr's socket through `herdr status server` when `HERDR_SOCKET_PATH` is unset, so the launchd service can list and message agents."

**Owned paths:** src/herdr_socket.rs, src/web/mod.rs, plus the nine release files: CHANGELOG.md, Cargo.toml, Cargo.lock, README.md, herdr-plugin.toml, package.json, package-lock.json, plugins/dogsled/.claude-plugin/plugin.json, plugins/dogsled/.codex-plugin/plugin.json.

**Acceptance.**
- `cargo test herdr_socket` and `cargo test web::` pass.
- `cargo clippy --all-targets -- -D warnings` and `cargo fmt --check` pass.
- Push branch `fix/web-herdr-socket`, open a PR, and report its number and SHA.
- Do not create or push a tag.

**Checks:** run the targeted tests and clippy only. CI runs the full suite.
