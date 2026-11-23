# end_turn: one tool that says how a turn ends and what it says

## Problem

The portal shows the newest assistant text of a group as its reply. Everything before that text folds into "Worked for…".

After an agent answers, a nudge often starts another short run. The nudge can be a `todo-reminder`, a dogsled check-in, a ticket, or a message. The agent's reply to the nudge becomes the newest text, and the real answer goes into the fold.

In 150 recent Pi sessions, 431 turns had text. In 112 of them a `todo-reminder` came after the first answer, and in 66 a dogsled delivery did. Examples:

- `2026-11-23T10-20` (this repo): the answer "I added `dogsled prune --path <checkout>`…" is hidden. The portal shows "Both of these are the two `cargo` jobs I ran…".
- `2026-11-20T15-02`: "The sidebar changes are merged in PR #409" is hidden under "No action needed. All five jobs are closed…".

`activity::reply` (`src/activity.rs:141`) uses the same "newest text" rule. So `read`, the inbox and the unread dot have the same problem.

## Decisions (owner, form `r-1795436138615-1271613-5306`)

1. The message is an argument of the tool call: `end_turn({ kind, message })`.
2. One tool replaces `done`, `ask` and `blocked`. `kind` is one of `reply | done | ask | blocked | waiting`.
3. Text that comes later in the same group collapses under "N follow-ups".
4. A newer pinned message replaces the old one only when the agent calls `end_turn` again.
5. The todo reminder does not fire after a successful `end_turn`.
6. If a group has no `end_turn`, each system nudge starts a new group. Then each answer stays the reply of its own group. This also fixes old sessions.

## The tool

`end_turn` takes two required parameters: `kind` and `message` (Markdown, not empty).

| kind | Effect in dogsled | Who can use it |
| --- | --- | --- |
| `reply` | No lifecycle signal. The turn ends with this message. | Everyone |
| `waiting` | No lifecycle signal. Refused when the caller owns no open children or queued work. | Everyone |
| `ask` | Same as today's `ask`: `done::signal(Ask, message)`. | Children (leads refuse, as today) |
| `blocked` | Same as today's `blocked`: `done::signal(Blocked, message)`. | Children (leads refuse, as today) |
| `done` | Same checks as today's `done::mark`. The message is the final reply, saved on the mark at once. | Children (leads refuse, as today) |

A refused call returns an error. The turn does not end, the message is not pinned, and the agent can fix the cause and call again.

### Where it lives

- **Rust core:** a new `done::end_turn(ledger, shell, pane, kind, message, now)` function routes to `mark` or `signal`, and does the `waiting` check. A new MCP `end_turn` tool and a CLI `dogsled end-turn --kind <kind> --message <text>` both call it.
- **Removed:** the MCP `done`, `ask` and `blocked` tools.
- **Kept:** the CLI `dogsled done`, `ask` and `blocked` commands, for scripts. Their help text points to `end-turn`.
- **Pi:** `pi/index.ts` registers a native `end_turn` tool that runs `dogsled end-turn` and returns `terminate: true`. Pi then makes no extra model call after it (`docs/extensions.md`, "Early termination"). Its `renderCall` shows the message as Markdown in the TUI. The MCP server entry sets `toolExposure: { end_turn: "hidden" }`, so Pi agents see only the native tool.
- **Claude and Codex:** use `mcp__dogsled__end_turn`. MCP cannot stop the turn, so the tool result says "Turn ended. Write nothing more." The portal treats later text as follow-ups.
- **Contract text:** `done::CONTRACT`, the MCP tool descriptions and server description, the Pi server description, and the watchdog `STOPPED_PROMPT` all change to one rule: "End every turn with `end_turn`."
- **Todo gate:** `pi/todo/src/gate.ts` allows `end_turn`, the same way it allows `mcp__dogsled__done` today.

## Reading it back

### Transcript reader (`src/activity.rs`)

A successful `end_turn` call becomes one `Turn` with `role: Assistant`, `text: message`, and a new field `end_turn: Some(kind)`. The reader matches the Pi native name `end_turn` and the MCP name `mcp__dogsled__end_turn`. It hides the raw tool call and its result, because they are not work.

A call whose result is an error stays an ordinary tool record and is not pinned.

`portal-types` `Turn` gets `end_turn: Option<EndTurnKind>` with `#[serde(default)]`. Older readers then still decode new snapshots.

### Final reply (`activity::reply`)

The final reply is the newest successful `end_turn` message in the newest owner group. If there is none, it falls back to the newest assistant text after the last nudge (decision 6). The sweep, `read`, the inbox and the unread dot all use this function. So the reply identity stays the same through cruft, and the unread dot does not fire again for a reply to a check-in.

### Seeded unit return (`src/unit_reply.rs`)

Today the return walks back from the leaf to the seal. It refuses a tool-call message as the final reply, and it needs `stopReason: "stop"`.

It will also accept this tail: an assistant message with one `end_turn` call with `kind: "done"`, then its successful result, then nothing more. The reply text is the call's `message`. All the other boundary checks stay.

## Portal (`web/src/agent_view.rs`, `Conversation`)

Groups still start at owner messages. In a finished group:

1. **The group has a successful `end_turn`:** show the owner message, then the collapsed work up to the newest `end_turn`, then the pinned message. After that, put all later records (nudges, text, tools) in one collapsed "N follow-ups" section. N counts the nudges and assistant messages in it.
2. **The group has no `end_turn`:** each displayed system nudge starts a sub-group. A nudge is a `Context` turn from `Source::Custom` or `Source::Dogsled`. Each sub-group keeps today's layout: head, collapsed work, last text.

The newest group stays fully expanded while the agent is active, as it is today.

The pinned message gets a small kind label when the kind is not `reply`: "Done", "Question", "Blocked", or "Waiting".

## Todo reminder (`pi/todo/src/lifecycle.ts`)

`settle` returns `{}` when the run's newest assistant tool call is a successful `end_turn`. In every other case it works as it does today.

## Tests

Each test uses real transcript lines, with no mocks of the portal or Pi.

- `src/activity.rs`: one Pi fixture with the shape of `2026-11-23T10-20`: answer, `end_turn`, check-in, cruft text. The test asserts that `reply` returns the `end_turn` message. A second fixture without `end_turn` asserts the fallback.
- `src/unit_reply.rs`: an `end_turn(done)` tail after the seal is accepted, and its message is the receipt text.
- `src/done/tests.rs`: `end_turn(waiting)` with no open work is refused, and `end_turn(done)` saves the reply on the mark.
- `web/src/component_tests.rs`: the pinned message and "N follow-ups" layout, and the nudge split when there is no `end_turn`.
- `pi/todo`: the reminder does not fire after a successful `end_turn`, and still fires after a refused one.
- Update the existing tests that name the MCP `done`, `ask` or `blocked` tools or the old contract text.

Before landing, I will check one real Claude and one real Codex transcript that call `mcp__dogsled__end_turn`. This confirms the tool name and the result shape that the reader matches.

## Work split

I will do it myself in this session, on `feat/end-turn`, in this order:

1. Core and MCP/CLI.
2. Reader, wire type, `reply` and `unit_reply`.
3. Portal.
4. Pi tool, gate and reminder.
5. Docs: `README.md`, `docs/lifecycle.md`, `docs/unit-requests.md`, and the skill's `references/commands.md`.

The parts share one wire field and one CLI command, but together they fit in one session.

## Out of scope

- Stopping check-ins about jobs the agent already stopped from starting a turn. This is a separate change.
- Worker profile prompts in the dotfiles that name `done`, `ask` or `blocked`. I will update them only after the release is published, as the release gate requires.

## Delivery

Run the moon checks locally, open a PR, and merge it to `main`. The change ships with the next release.

## Implementation notes

- The `done` mark keeps the call's message in a new column, `end_turn_message` (store version 8). The handoff prefers the sealed unit receipt, then this message, then the transcript. Setting `mark.reply` early would have changed the meaning of `native_done`, so the plan's "save the reply on the mark" uses its own field.
- Codex in code mode calls MCP tools from an `exec` script, so its transcript has no separate `end_turn` call. Real rollouts show `tools.<server>__<tool>(…)` inside `exec`. For Codex the portal and `read` use the fallback, but the `done` handoff uses the saved message. No logged-in Claude or Codex is available on this host, so the live transcript check is still open.
- Claude plugin servers name tools `mcp__plugin_<plugin>_<server>__<tool>`. The reader accepts any `mcp__…dogsled__end_turn`.
