# Close agent sessions from the portal

## Outcome
The owner can close the selected agent terminal from the portal, including a working agent, on local or remote hosts.

## Cause and decision
`web/src/agent_view.rs:653` exposes Interrupt and Open in Herdr, but no session close control.
`src/web/agents.rs:551` and `web/src/api.rs:139` likewise lack a close action.
The user rejected guarded Dogsled cleanup: “it should just be like if i close a herdr tab.”
Close the selected pane immediately. Do not close sibling panes, call `runtime::close`, remove worktrees, or mark tasks complete.
Keep native Herdr refusals. Do not bypass worktree-group confirmation or retry uncertain mutations.

## Edits
- `src/herdr_socket.rs`: add the narrow `pane.close` client method beside `focus_pane`, with a fake-socket contract test.
- `src/web/agents.rs`: add POST `/api/agents/{host}/{pane}/close`, without a request body, under the existing origin guards.
  Local requests call the socket method. Remote requests extend `MachineAction` and use `herdr --machine <resolved-label> pane close <pane>`.
  Keep the existing action host resolution. Never fall back to local or use `agent close`.
  Return 202 only after success. Reuse existing JSON error envelopes and unavailable/missing-pane mappings.
  Test the real router with local success, working/blocked states, missing pane, unavailable socket, foreign origin, and remote success/failure.
  Assert the exact target and command. Assert no local close on remote failure, no activity gate, and no Dogsled cleanup effects.
- `web/src/api.rs`: add `close_agent` through the existing host-qualified URL and error handling.
- `web/src/agent_view.rs`: add a secondary icon button at the right of the existing header actions.
  Use the official Button, existing X icon, and `aria_label`/title `Close session`. Keep the existing console layout and styling.
  Close on click, without an idle prerequisite or an interrupt-first flow. Disable during a pending request or unavailable metadata.
  On success, clear the selected agent and matching inbox selection only if they still identify the requested host and pane.
  On failure, retain the view and display the existing action error. Do not hide failed closes or remove rows optimistically.
  Test enabled working/blocked agents, disabled unavailable/pending states, success selection handling, and failure retention through production behavior.

## Paths
Own only the four source files above. Tests stay in their existing inline modules.
Read `web/src/app.rs`, `web/src/components/`, `web/assets/console.css`, `src/web/guard.rs`, `moon.yml`, `DESIGN.md`, and related callers.
Do not change dependencies, cleanup policy, shared lifecycle state, installed binaries, configuration, or generated files.
This is an addition to the existing header, not a new layout. No wireframe or redesign is needed.

## Acceptance
A single click closes the exact selected terminal on its host, regardless of agent activity, without invoking checkout cleanup.
A failed close stays visible. Another selected agent is not cleared by a late response.
Provide focused test results with the final commit SHA, the scoped diff, and any unavailable verification.
Use only fake transports or disposable fixtures. Do not close existing user or worker sessions for testing.
Verify the rendered header at 1600px and the supported 800px minimum when the repository-managed browser is available.

## Checks
Run `cargo fmt --check`, `cargo test -p dogsled web::agents`, and `cargo test -p dogsled herdr_socket`.
Run `cargo test -p web` and `cargo clippy -p web --target wasm32-unknown-unknown -- -D warnings`.
Keep repository commit hooks enabled. Do not run additional full suites except those required by hooks.
Read the applicable code-standard and UI skills. Run the Impeccable detector once on changed UI targets after implementation.

## Base and research evidence
Base: `bf591f1cc18a85df96eb1c26008141bd57595e35` (`origin/main` when inspected).
Installed Herdr client and server: 0.9.5. `herdr api schema --json` requires `{pane_id: string}` for `pane.close`.
Primary implementation: https://github.com/herdrdev/herdr/blob/7573e821895d32c8d1f78b3f6c9a2f85a46df4ac/src/app/api/panes.rs (`handle_pane_close`, `close_pane`).
It shuts down detached terminal runtimes and retains native restored/worktree-group refusals. It has no idle-state prerequisite.
Nearby Dogsled examples: `src/web/agents.rs:395-460` (machine actions), `:1429-1455` (remote routing tests), and `web/src/agent_view.rs:667-681` (header action).
Rejected alternatives: guarded Dogsled close, automatic interrupt-and-wait, tab-wide closure, and checkout cleanup.
