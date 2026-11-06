# U4: the agent view refetches only changed parts

Base: `c8b2147` (`integrate/files-refresh`: `main` `73dea6c` with LR2, then U1, U2, U1b, U3). Line numbers are at that base.

## Outcome

The open agent view makes no request when nothing changes. A conversation change refetches only the conversation. A screen change refetches only the screen, and only while the agent is blocked. Agent metadata, the board and the inbox come from the portal stream that `App` already holds. A prompt or key press makes exactly one request. While the agent stream is down, the view says so and reconnects with backoff. On each new connection it refetches every part once.

## Cause

`AgentSession` (`web/src/agent_view.rs`):
- One `refresh` counter (`:135`) drives six fetches: the conversation (`:149`, `conversations.rs:142-170`), the agent (`:151-171`), the screen (`:173-205`), and the board, reviews and questions (`:207-232`).
- Every agent event bumps the counter (`:262`). So does a 5 s `use_interval` (`:269`), each prompt (`:309`), each key press (`:326`) and each answered question (`:512`).
- `send` fetches the whole agent list before each prompt (`:282-292`). U3's server guard makes that fetch redundant.
- `watch_agent` runs once (`:234-266`). A failure or clean close stops live updates for good, and `:605` claims "Checking every 5 seconds".
- `App` already receives board, agents and inbox on `/api/stream` (`web/src/app.rs:302-304`, `:346-353`). The view fetches them again through `/api/board` and `/api/agents`.

## Edits

1. **`web/src/app.rs`:** provide one context value holding the existing memos for agents, the agent error, the board and the board error. Make no other change.
2. **`AgentSession`, agent and board:**
   - The agent is the entry in the portal `agents` that matches the host and pane. Reachable means the agent error is `None`.
   - Overlay `state` and `blocked` from the newest `AgentEvent` of the current connection, as `:252-260` does now.
   - Delete `agent`'s fetch, `metadata_loading`, the `board` signal, `board_error` and the board fetch.
3. **`AgentSession`, parts:**
   - Hold `Option<DetailRevisions>` for the current connection.
   - Add one pure function that maps (held revisions, event, blocked) to the parts to fetch:
     - Held `None` (the first event of a connection): fetch the conversation, plus the screen when blocked.
     - Otherwise: fetch the conversation when `revisions.conversation` changed. Fetch the screen when blocked and either `revisions.screen` changed or the agent just became blocked.
     - An event without `revisions` (an older peer) fetches both parts.
   - Drive `use_conversation` from a conversation trigger, not `refresh`. Keep the session fencing.
4. **`AgentSession`, reviews and questions:** refetch the review and question lists only when the set of inbox ids for this agent changes. Use the `inbox` prop (`InboxItem.selection` host, pane and id). An answered question removes its item locally, as now, with no refetch.
5. **`AgentSession`, stream:**
   - Run `watch_agent` in a loop. On an error, or on the "stream ended" result, set the stream error, clear the held revisions, and wait with backoff: 1 s, doubling, at most 30 s, reset by the first event.
   - Use `dioxus_sdk_time::sleep`, as `app.rs` does.
   - Change `:605` to "Live updates paused. Reconnecting."
6. **`AgentSession`, actions:**
   - Delete `refresh`, `POLL` and `use_interval`, together with every `refresh += 1`.
   - `send` calls `api::send_prompt(&pane, &message, prompt_reply)` directly, and shows a 409 message through `action_error` as it shows any other error.
7. **`web/src/api.rs`:** delete `fetch_agent`, `fetch_agents` and `fetch_board` if nothing else uses them.

Do not edit `Conversation`, `ToolRecords`, `LiveActivity`, the right-column tabs, `review_view.rs` or `question_form.rs`.

## Paths

Owns:
- `web/src/agent_view.rs` (`AgentSession` data hooks, actions, the `:605` status and its tests)
- `web/src/conversations.rs` (`use_conversation`)
- `web/src/api.rs`
- `web/src/app.rs` (the context value only)
- `web/src/component_tests.rs`

Reads: `portal-types/src/agents.rs`, `src/web/agents.rs`.

## Acceptance

- Unit tests of the parts function:
  - the first event of a connection;
  - a screen-only change while blocked, which fetches the screen only;
  - a screen-only change while not blocked, which fetches nothing;
  - a conversation change;
  - a transition to blocked;
  - an event without revisions.
- A unit test of the backoff sequence and its reset.
- `rg -n 'refresh|POLL|use_interval|fetch_agent|fetch_board' web/src/agent_view.rs` finds nothing.
- `cargo test -p web` passes.

## Checks

- Run `cargo fmt --all --check`.
- Run `cargo clippy -p web --all-targets -- -D warnings`.
- The lead prebuilds `web/public` and runs `moon run dogsled:web-lint`. Do not run `web-build`.
- Commit through the pre-commit hook. Do not bypass it.
- The brief is complete. It does not depend on any plan file.
