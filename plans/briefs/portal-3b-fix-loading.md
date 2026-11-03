# Brief: agent conversation never leaves "Loading conversation…"

**Outcome.** When an agent is selected in the portal, its conversation turns appear within a second. Today the centre column keeps showing "Loading conversation…" forever.

**Evidence.** The lead checked this on main at `fafd1e7`, using `dogsled web --port 7466` and headless Chrome on `/?pane=w83:p9B`.
- The header renders, so `api::fetch_agent` succeeds.
- After 20 s, the DOM still contains "Loading conversation…" and no turns. The screenshot is `/tmp/pv-agent.png`.
- The server is not the cause:
  - `GET /api/agents/w83:p9B/conversation` answers 200 in 3 ms with 1 turn.
  - `/api/agents` answers in 2 ms.
  - `/api/agents/w83:p9B/events` sends 0 events in 8 s.

**Where to look.** `web/src/agent_view.rs`, in `AgentSession`:
- The `use_resource` at line 62 runs `fetch_agent`, `agent.set`, then `fetch_conversation`, and only then `loading.set(false)`.
- `loading` is never cleared if that future is dropped or restarted before it finishes. In Dioxus 0.7, `use_resource` reruns when a signal read inside the future changes, and a rerun cancels the running future.
- Find what restarts it or stops it from finishing. Candidates are signal reads inside the future, and the board SSE re-rendering `App` every 2 s. Confirm the cause before you change anything.
- Fix the cause. Do not hide it with a timeout or by clearing `loading` early.

**Reproduce.**
- The lead keeps a built tree at `/Users/oisin/dev/dogsled/.worktrees/verify-portal-main`, with a debug binary and the web bundle.
- `sh /tmp/portal-conv.sh` starts the server, times the routes and counts events.
- `sh /tmp/portal-agent.sh` starts the server and takes the screenshot.
- You may run those scripts and the existing binary. Do not run full builds. When you need a rebuilt bundle to check your fix, ask the lead and say which commit to build.

**Owned paths:** web/src/agent_view.rs.

**Acceptance.**
1. `cargo fmt --check -p web`, `cargo clippy -p web --target wasm32-unknown-unknown -- -D warnings` and `cargo test -p web` pass.
2. Report the cause, with the evidence that confirms it.
3. Commit, push the branch and open a draft PR. Report the PR number and SHA. The lead then rebuilds and takes the screenshot again.
