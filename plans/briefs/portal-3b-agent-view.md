# Brief: agent view and blocked-prompt UI (web portal step 3b)

**Outcome.** When you click an agent row on the board, the centre column shows that agent with:
- Its live conversation.
- A message box.
- A blocked panel, when the agent is blocked. The panel shows the agent's screen and lets you answer it.

**Server API.** The routes are on main in `src/web/agents.rs`. The types are in `portal-types/src/agents.rs`.

| Route | Request | Response |
|---|---|---|
| `GET /api/agents` | none | `AgentList{agents:[AgentView{pane,name?,agent?,state,cwd?,blocked?:BlockedPrompt{question,options:[{key,label}]}}]}` |
| `GET /api/agents/{pane}/conversation` | none | `Conversation{pane,turns:[Turn{role,text,..}]}` |
| `GET /api/agents/{pane}/screen` | none | `Screen{pane,text,revision}` |
| `POST /api/agents/{pane}/prompt` | `PromptBody{text}` | — |
| `POST /api/agents/{pane}/keys` | `KeysBody{keys}` | — |
| `POST /api/agents/{pane}/focus` | none | — |
| `GET /api/agents/{pane}/events` | none | SSE stream of `AgentEvent{kind,pane,state?,blocked?}` |

- `keys` accepts only these keys: one printable character, enter, esc, tab, backspace, the arrow keys and ctrl+c.
- For the SSE event name, read the route code.
- The page sends its POSTs from the same origin, so the Origin guard accepts them.

**Existing UI code.**
- `web/src/app.rs` is the page shell. It provides a `Signal<Option<String>>` context with the selected pane, and renders `AgentView { pane }` in the centre.
- The board is in `web/src/board.rs`.
- The API helpers are in `web/src/api.rs`. They use gloo-net and a `BASE` const.

**Edits.**
- **`web/src/board.rs`:**
  - A click on an agent row sets the selected pane.
  - The selected row is highlighted.
  - A row without a pane is not clickable.
- **`web/src/agent_view.rs`:**
  - **Header:** name, agent kind, state pill, cwd, and an "Open in Herdr" button that calls focus.
  - **Conversation:**
    - Show the turns oldest first. Give each role its own style.
    - Scroll to the bottom on new turns, unless the user has scrolled up.
    - Fetch the conversation again on each event, and every 5 s as a fallback.
  - **Message box:** a textarea. Enter sends to `/prompt`; Shift+Enter adds a newline.
  - **Blocked panel:** when the agent or the prompt result reports a block, show the panel instead of sending. It holds:
    - The `/screen` text, in a monospace pre.
    - One button for each `BlockedPrompt` option, which sends that option's key.
    - Buttons for enter, esc, the arrow keys and ctrl+c.
    - A short text input that sends its characters, then enter.
  - **Shared draft:**
    - Add `pub struct Draft(pub Signal<String>)` in this file.
    - When `try_use_context::<Draft>()` finds that context, the message box uses it for its text.
    - The review unit will provide this context, to put quotes into a message.
- **`web/src/api.rs`:** add the agent calls.
- **New `web/assets/agent_view.css`:**
  - Load it with `document::Stylesheet` in AgentView.
  - Reuse the colour variables from `main.css`. Do not edit `main.css`.

**Owned paths:** web/src/agent_view.rs, web/src/board.rs, web/src/api.rs, web/assets/agent_view.css.

**Acceptance.**
1. The checks exit 0.
2. Run the page against the live Herdr. Use `dogsled web`, or `dx serve` with its `/api` proxy.
3. Take three screenshots with headless Chrome or Playwright, and report their paths:
   - The board with an agent selected.
   - That agent's conversation.
   - The blocked panel. If no agent is blocked, render the panel with a stub `BlockedPrompt`, and say so.
4. Send one real message to a test agent, and show that it arrived. Spawn the test agent yourself, for example a Herdr shell pane. Never send to another worker.
