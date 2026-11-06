# LR1: reasoning and tool detail on the wire

Plans: `plans/conversation-live-work-mockups.md` ("Data the implementation needs"), `plans/lead-workers-tab.md`. Base: `origin/main` `aef6cce`.
Mockups: `git show origin/docs/portal-mocks:.impeccable/mocks/lead-workers-tab/lead.html`.

## Outcome

The portal receives the agent's thinking text and tool call lines live from Pi, and thinking blocks, tool call lines, and tool output tails from transcript history.

## Cause

On 2026-11-06 the owner reversed the rule that kept reasoning and tool arguments out of the portal. The old rule is enforced here:
- `pi/activity.ts:389-405` drops `thinking_delta` text; `:411-420` drops `event.args`.
- `portal-types/src/agents.rs:247-269`: `ActiveTool` and `LiveState` have no text fields and deny unknown fields.
- `src/activity.rs:576-612` (`tool_record`) and `:622-656` (`records`) skip `thinking` blocks, call arguments, and result content.
- `src/observation/live_activity.rs:604-641` validates the live state.

## Edits

1. One call-line rule, in Rust (`src/activity.rs`, new `pub fn call_line(name, arguments) -> String`) and TypeScript (`pi/activity.ts`, `callLine`). Arguments may be a JSON string (Codex). A `command`/`cmd` field gives `$ <command>`. Otherwise a `path`/`file_path` field gives `<lowercase name> <path>`. Otherwise `<name> <compact JSON arguments>`. Keep newlines. `Step::line` (board) stays as it is.
2. `portal-types/src/agents.rs`:
   - `LiveState`: add `reasoning: Option<String>` (serde default, skip if none). It is the thinking text of the in-flight assistant message.
   - `ActiveTool`: add `call: Option<String>` (same serde attributes).
   - `Role`: add `Thinking`. A thinking turn has the block text as `text`.
   - `ToolSummary`: add `call: Option<String>`, `output: Option<String>` (the last 20 lines of the result text), `output_lines: Option<u32>` (total result lines). Same serde attributes.
   - Rewrite the doc comments that say reasoning and arguments never cross.
3. `pi/activity.ts`: the `block` event carries the thinking `delta`. The reducer appends deltas per content index and clears them at `message_end` and `settled`. `snapshot()` sets `reasoning` to the joined thinking text, newest 16 KiB, cut on a character boundary, or omits it when empty. `tool_start` sets `call` from `callLine(event.toolName, event.args)`, newest 512 bytes.
4. `src/observation/live_activity.rs` `validate_publication`: reject `reasoning` over 16 KiB and a `call` over 512 bytes. Keep all other checks.
5. `src/activity.rs`:
   - `records` emits a `Role::Thinking` turn for each Pi or Claude `thinking` block with non-empty text, in block order. Id: `<message id>:thinking:<block index>`. Cut text over 32 KiB to the first 32 KiB and set `history.truncated`.
   - Give thinking turns their own bound, `CONVERSATION_THOUGHTS = 400`, so they cannot evict text or tool turns.
   - `tool_record` sets `call` from the call block (Pi `arguments`, Claude `input`, Codex `arguments`), at most 4 KiB. A result sets `output` (at most 4 KiB) and `output_lines` from its text: Pi `content[].text`, Claude `content` string or text blocks, Codex `output`.
   - In `conversation_with_limit`, merging a call with its result keeps the call's `call` and the result's `output` and `output_lines`.
6. Tests. Reverse the tests that assert privacy:
   - `src/activity.rs:1207`: payloads now appear, within the bounds.
   - `pi/index.test.ts:188`: the thinking delta reaches the published payload.
   - Update the constructors in `src/web/activity.rs`, `src/observation/tests/live_activity.rs`, `src/observation/ipc/tests.rs`, `web/src/activity.rs`, `web/src/component_tests.rs`. Add only the new fields there.
   Add: one shared table of call-line cases, asserted in Rust and TS; a reducer test for delta accumulation, the 16 KiB tail, and clearing; a validation test for each new bound; a Pi and a Claude transcript fixture with thinking, call, and result.

## Paths

Owns: `portal-types/src/agents.rs`, `src/activity.rs`, `src/observation/live_activity.rs`, `src/observation/tests/live_activity.rs`, `src/observation/ipc/tests.rs`, `src/web/activity.rs`, `pi/activity.ts`, `pi/activity.test.ts`, `pi/index.test.ts`, `web/src/activity.rs`, `web/src/component_tests.rs` (constructors only), `web/src/agent_view.rs` (only the `Role` match at line 728: `Role::Thinking => "Thinking"`).
Reads: `src/`, `pi/`, `web/src/`. Do not change UI rendering otherwise; LR3 owns it.

## Acceptance and checks

`cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`, `cargo clippy -p web --target wasm32-unknown-unknown -- -D warnings`, `pnpm run --silent typecheck`, `node --test pi/activity.test.ts pi/index.test.ts` all pass. (`cargo` needs `web/public`; run `moon run dogsled:web-build` first if it is missing.)
Open a PR to `main`.
