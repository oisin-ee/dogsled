# U3: the prompt route refuses a blocked agent

Plan: `plans/git-files-sidebar.md`, unit U3. Base: `22d1009` (`fix/publication-dedupe`: U1, U2, U1b).

## Outcome

`POST /api/agents/{host}/{pane}/prompt` checks the agent's live Herdr state on its own host. If the agent is blocked and the request is not a reply to its prompt, the route answers 409 and makes no prompt call. This is true for local and remote agents. The browser then no longer needs to fetch the agent list before it sends; U4 removes that fetch.

## Cause

- `prompt` (`src/web/agents.rs:742-760`) sends without checking state. The only guard is in the browser: `send` (`web/src/agent_view.rs:272-310`) fetches the whole agent list (`api::fetch_agent`, `web/src/api.rs:79-86`) before each prompt and refuses when the agent is blocked.
- `PromptBody` (`portal-types/src/agents.rs:208-210`) cannot say whether the text answers the blocked prompt. That is why the check lives in the browser.

## Edits

1. `portal-types/src/agents.rs` `PromptBody`: add `#[serde(default)] pub reply: bool`, with the doc comment "the text answers the agent's blocked prompt".
2. `src/web/agents.rs` `prompt`:
   - Local: before `socket.agent_prompt`, read `socket.agent_list()` and find the entry whose `pane_id` is `pane`. If there is none, answer 404 with `not_found(&pane)`. If its `agent_status` is `"blocked"` and `!body.reply`, answer 409 with `{"error":"The agent needs a response. Select its prompt in Waiting on you to reply."}`.
   - Remote: in the same `spawn_blocking` as the prompt, run `herdr --machine <label> agent get <pane>` first. Parse `/result/agent/agent_status` and apply the same rule before `herdr --machine <label> agent prompt`. Refactor `machine_action` so the prompt operation can run this check first; the keys and focus routes are unchanged.
   - Put the rule in one function used by both paths. A missing or unparsable status is a 502 that names the host, not a send.
   - Add a doc comment on `prompt` stating the remaining gap: Herdr's `agent.prompt` has no conditional form, so an agent that blocks between the check and the send still receives the text.
3. `web/src/api.rs` `send_prompt`: add a `reply: bool` parameter and put it in the body. `web/src/agent_view.rs` (its one call site, about line 291): pass `prompt_reply`. Make no other browser change; U4 owns the rest of `send`.

## Paths

Owns: `portal-types/src/agents.rs` (`PromptBody` only), `src/web/agents.rs`, `web/src/api.rs` (`send_prompt` only), `web/src/agent_view.rs` (that one call only).
Reads: `src/herdr_socket.rs`, `src/probe.rs`, `src/hosts.rs`.

## Acceptance

Add or extend tests in `src/web/agents.rs`:
- Local blocked agent, no `reply`: 409 with the message, and `server.methods()` has no `agent.prompt`. The same agent with `"reply":true`: 202, and `agent.prompt` was called.
- Local working agent: 202. Unknown local pane: 404, with no `agent.prompt`.
- Remote: the fake shell answers `herdr --machine netcup agent get w1:p1` with a blocked agent. Without `reply`: 409, with no `agent prompt` call. With `reply`: the prompt call happens. Update `remote_conversation_and_actions_target_the_named_peer` so its `agent get` answers a working agent.
- A body without `reply` still deserializes (`portal-types` test).

Run `cargo test --bin dogsled web::agents` and `cargo test -p portal-types`. Both must pass.

## Checks

`moon run dogsled:fmt-check` and `cargo clippy --all-targets -- -D warnings`. The lead prebuilds `web/public` and runs `moon run dogsled:web-lint`; do not run `web-build`. Commit through the pre-commit hook; do not bypass it.
