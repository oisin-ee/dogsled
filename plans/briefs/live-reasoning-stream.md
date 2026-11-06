# LR3: one-stream conversation with thinking and tool detail

Plans: `plans/conversation-live-work-mockups.md`, `plans/lead-workers-tab.md`. Needs LR1 (wire fields) and LR2 (agent view layout) merged. Base: `origin/main` after both merge.
Mockups: `git show origin/docs/portal-mocks:.impeccable/mocks/lead-workers-tab/lead.html` (stream markup and CSS), `lead-1600.png`, `lead-1600-history.png`.

## Outcome

The conversation reads like Pi: messages, thinking, tool calls, and replies in recorded order, with the in-flight thought streaming at the live edge.

## Cause

- `web/src/agent_view.rs` `Conversation` and `ToolRecords` fold each tool run into one row of names (`5 tool records · bash, bash, read`) and have no thinking turns.
- `LiveActivity`/`ActivityRow`/`activity_label` show one label (`Thinking…`) above the composer and ignore `LiveState.reasoning` and `ActiveTool.call`.

## Edits

1. `Conversation` (`web/src/agent_view.rs`): render turns in order, without folding tool runs into a name list.
   - `Role::Thinking`: italic, muted text, as Pi's TUI renders it. Markdown as for messages.
   - `Role::Tool`: the `call` line in monospace (`$ git status`, `read src/x.rs`). Below it, `output` in monospace. If `output_lines` exceeds the lines shown, a `N earlier lines` line comes first. An error result gets the error colour. A tool without `call` shows its name.
   - Messages keep their author, source, time gutter, and Markdown.
2. Finished turns: the records between an owner message and the agent's final reply of that turn fold into one line: `Worked for <span> · <t> thoughts · <c> tool calls`. The span runs from the owner message time to the newest record time of the turn; omit it if either time is missing. The line toggles the records open. The newest turn stays open while the agent is not idle.
3. Live edge (`LiveActivity`, `ActivityRow`): below the history, while `reasoning` is present, show it as the newest thought: italic, full contrast, following the scroll. Each running tool shows its `call` line. The footer row keeps the phase with elapsed time, and adds the turn's thought and tool counts, as in `lead-1600.png`. When the live frame clears `reasoning`, the transcript refetch supplies the finished thought; do not show both copies.
4. `web/assets/console.css`: `DESIGN.md` tokens only. No horizontal overflow in the conversation column at 900, 1280, and 1600px.
5. `web/src/component_tests.rs`: thinking turns, call and output rendering with the earlier-lines fold, error results, the folded `Worked for` line with and without times, live reasoning and running call lines, and no duplicate thought after message end.

## Paths

Owns: `web/src/agent_view.rs` (`Conversation`, `MessageRecord`, `ToolRecords`, `LiveActivity`, `ActivityRow`, `activity_label` and their helpers), `web/assets/console.css`, `web/src/component_tests.rs`.
Reads: `web/`, `portal-types/`, `DESIGN.md`. Do not change server code or the wire types.

## Acceptance and checks

`cargo fmt --check`, `cargo test -p web`, `cargo clippy -p web --target wasm32-unknown-unknown -- -D warnings` pass. The lead does the browser check.
Open a PR to `main`.
