# U4a: the agent view keeps a loading state before the first portal snapshot

Base: `b5eb1d8` (`feat/agent-view-parts`, U4). Line numbers are at that base.

## Outcome

When the portal opens directly on a selected agent, the view shows a loading state until the first portal snapshot arrives. It never shows "This agent is no longer available" or treats Herdr as reachable before that snapshot. When the snapshot arrives, the agent is shown as present, missing or unreachable from that snapshot.

## Cause

- U4 removed `metadata_loading` and now reads the agent from the portal stream: the `AgentPortal` context in `web/src/app.rs:27-34` and `:313-318`.
- `PortalState` (`web/src/app.rs:224-231`) holds `agents: Vec<Agent>`, which starts empty, and `agent_error: None`.
- So `AgentSession` (`web/src/agent_view.rs:116-145`) can't tell "not received yet" from "the agent is gone". Before the first snapshot it shows the Alert from `metadata_error`, sets `reachable` to true, and disables the composer because `current` is `None`.
- The view doesn't own this state. `PortalState` has no record of whether a snapshot has been received.

## Edits

1. **`web/src/app.rs` `PortalState`:** record whether a snapshot's `agents` has arrived. In `AgentPortal`, change `agents` to `Memo<Option<Vec<Agent>>>`:
   - `None` until the first snapshot that has `agents`;
   - after that, the last list received. A later stream error keeps the list and sets `agent_error`, as it does now.

   Leave the other uses of `agents` in `app.rs` unchanged: `:311`, `:410`, `:500` and `:526`.
2. **`web/src/agent_view.rs`:** add one pure function that maps (`Option<&[Agent]>`, agent error, address) to `Loading`, `Missing`, `Unreachable(message)` or `Present(agent)`. `AgentSession` derives `agent`, `reachable` and `metadata_error` from it.
   - **Loading:** no metadata Alert and no "Herdr is not reachable" status. The state label shows "Loading…". `LiveActivity`'s `connected` is true. The composer is disabled.
   - **Unreachable:** the same as now.
   - **Missing:** the same as now.
   - **Present:** apply the existing event overlay of `state` and `blocked`.

Do not change any other part of U4.

## Paths

- **Owns:** `web/src/app.rs` (`PortalState`, `AgentPortal`), and `web/src/agent_view.rs` (`AgentSession` metadata derivation, plus its tests).
- **Reads:** `portal-types/src/agents.rs`.

## Acceptance

- Unit tests of the pure function, one per case: Loading, Missing, Unreachable and Present.
- A `PortalState` test:
  - before any update, the agents are `None`;
  - after a snapshot, they are `Some`;
  - after a later stream error, the list is kept and the error is set.
- `cargo test -p web` passes.

## Checks

- Run `cargo fmt --all --check` and `cargo clippy -p web --all-targets -- -D warnings`.
- The lead runs `moon run dogsled:web-lint`.
- Commit through the pre-commit hook as a new commit; do not amend and do not bypass the hook.
- This brief is complete. It depends on no plan file.
