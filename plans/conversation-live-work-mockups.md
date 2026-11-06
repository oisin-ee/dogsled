# Conversation live-work mockups

## Scope and status

Status: one chosen direction, revised for owner review. No production behavior, producer, or data contract changed.

On 2026-11-06 the owner reviewed three directions and chose **one stream like Pi**.
The owner also wanted the focus idea (each working agent showing what it is doing now) in a list, not in a card row across the top.
The split and focus directions were removed; Git history keeps them (`84e045d`).

On 2026-11-06 the owner chose:

1. **Option B, live reasoning stream.** The agent's reasoning text streams in real time.
2. **Tool calls show their arguments the way Pi and Herdr show them now.** No separate allowlist or redaction layer.
3. **The conversation needs more space.**

The owner rejected the earlier mockups in `.impeccable/mocks/conversation-live-work/`: an inline block plus a dock, drawn in the old timeline look, with invented content.
They are superseded and are not a starting point. The new directions are in `.impeccable/mocks/live-reasoning/`.

## What the previews show

The conversation plays back a real Pi turn, from `~/.pi/agent/sessions/--Users-oisin-dev-dogsled--/2026-11-06T11-53-15-658Z_*.jsonl`:

- The owner asked "why is this happening?" with a screenshot at 11:53:45. The claude-opus-5-5 lead worked for 2m 31s (6 thinking blocks, 13 tool calls) and replied at 11:56:16.
- The owner wrote "no, give me proper responses, not some handwavey shit." at 12:26:00.
- The previews freeze at 12:27:19, mid-thought, after 8 thinking blocks and 8 tool calls of the second turn. That last thought types in at the live edge.

The text is the recorded text: the thinking blocks, the exact bash commands, the output tails, timestamps, and durations from tool-result timestamps.
Only the agent list, host meters, and todo column are illustrative.
Thinking renders italic and muted, as Pi's TUI renders it. The newest paragraph is full contrast.
Bash calls render as `$ command` with the last lines of output and an "N earlier lines" fold, as Pi does.
A finished turn folds its work into one line, `Worked for 2m 31s · 6 thoughts · 13 tool calls`, above the reply. Opening it shows the full recorded work.

## The chosen direction

`one-stream.html`:

- **Conversation:** one stream, like Pi. Messages, thinking, tool calls, and replies in recorded order.
- **Left sidebars:** unchanged from the shipped portal. The inbox (280px) and the agents list (248px) are two separate columns, per the 2026-11-02 steer.
  Todos sit on the right from 1400px and become a tab below it. Below 1100px, agents become a tab, as shipped.
  The owner rejected stacking agents under the inbox on 2026-11-06.
- **Agent rows:** grouped by repository, as shipped. A working lead row adds two things:
  - its phase and elapsed time, such as `Thinking · 1m 19s`;
  - its newest thought (italic, two lines) or its running command (`$ …`, monospace, two lines).
  The selected row has an indigo edge, and its snippet follows the live stream. A row that is not working keeps one line of state.
- **Workers:** each lead's workers nest under it, as the shipped `RepoGroup` does with `SidebarMenuSub` (`web/src/board.rs`).
  Each worker row has three lines: the title, its phase and elapsed time, and its newest thought or command (one line).
  Clicking a worker opens its own stream in the conversation column.
  Change from shipped: a running worker gets the green dot. Today only leads do (`board.rs` `NavigationAgent`).
- **Agent data:** the rondo group is the real fleet at 13:24 on 6 Nov.
  The claude-opus-5-5 lead had spawned three gpt-6.1-sol workers at 13:15. Their titles, phases, thoughts, and commands come from the lead's Pi session and the workers' sessions in `~/.local/state/dogsled/launches/`.
  The dogsled row is the replayed conversation. The two moments are 57 minutes apart; the mockup shows them together.
- **Space:** with the shipped columns, the conversation is 772px at 1600px wide. This mockup does not gain width. That is an open question for the owner.

Captures (each `.png` has a `.png.json` provenance sidecar):

- `one-stream-1600.png`, `one-stream-1280.png`, `one-stream-900.png`: live edge at 1600 × 1000, 1280 × 800, and 900 × 800.
- `one-stream-history.png`: the top of the conversation with the earlier turn's work opened.

Review through Impeccable:

```sh
/Users/oisin/.pi/agent/skills/impeccable/scripts/impeccable serve-question --start --payload .impeccable/mocks/live-reasoning/review.json
```

The HTML files work without that server. Add `?still` to the URL to stop the playback. Controls are disabled.

## Why the current view looks idle

- `pi/activity.ts:393-404` turns every `thinking_delta` into a bare `thinking` phase and drops the text.
- `src/activity.rs:142,196` and `portal-types/src/agents.rs:131,245` drop reasoning and tool arguments from history and live state.
- `web/src/agent_view.rs:764-800` folds tool records into one row of names, such as `5 tool records · bash, bash, read`.
- `web/src/agent_view.rs:914-945` shows the live state as one label above the composer.

The data exists. A scan of recent Pi transcripts in this repository on 2026-11-06 found:

| Model | Steps with reasoning text | Shape |
| --- | --- | --- |
| gpt-6-astra | 55 of 56 | Short bold headlines, such as `**Inspecting sidebar tabs**` (median 57 characters) |
| claude-opus-5-5 | 38 of 39 | Prose (median 579 characters, longest 2,788) |

The replayed turn shows both shapes: its first thoughts are headlines (`**Checking project files**`), and its later thoughts are prose.

## Data the implementation needs

| Display | Source | Current gap |
| --- | --- | --- |
| Agent-list snippet | The same live reasoning text, or the running tool call, per agent | Agent rows carry only a phase today |
| Live reasoning text | Pi `message_update` `thinking_delta` events | Publisher drops `delta`; `LiveState` has no text field and denies unknown fields |
| Finished thoughts in history | Transcript `thinking` blocks | Transcript reader drops them |
| Tool call line | Transcript `toolCall` arguments; Pi `tool_execution_start` args | Not read or published |
| Tool output tail and status | Transcript tool results; Pi `tool_execution_end` | Only completed/error/unknown is kept |
| Durations | Assistant and tool-result timestamps | Not read |

Live frames are latest-value only. A viewer that connects mid-turn must get the turn's earlier thoughts from the transcript, not from the stream.
Reasoning and output need size bounds per block and per frame. The bounds are an implementation decision.
Claude and Codex harnesses keep the coarse "Working" state until a producer exists for them.
The portal is Rust, so it cannot call Pi's TypeScript `renderCall`. It must reproduce the built-in formats (`$ command`, `read path`, `edit path`) and fall back to the tool name plus arguments.

## Method and evidence

- Impeccable context was loaded for `web/src/agent_view.rs`. The previews use the `DESIGN.md` palette, type, radii, and segmented-tab tokens.
- The generator read the session JSONL and wrote the HTML. It is a one-off design script and is not in the repository.
- The managed browser rendered `one-stream.html` at 1600 × 1000, 1280 × 800, and 900 × 800. No scroll region, header, top bar, agent row, worker row, or phase line had horizontal overflow at any size.
- In playback, the selected row's snippet advanced with the typed thought.
- The Impeccable detector found `overused-font` (Inter, the `DESIGN.md` font) and `flat-type-hierarchy` (the portal's 13–14px Operate scale). It found no other issues.

## Before implementation

1. The owner approves the revised direction, including the worker rows, and decides how, if at all, the conversation gets more width.
2. Update the editable Squig layout (`plans/chat-conversation.squig.json`) to it.
3. Define the protocol change: reasoning and tool-argument fields, bounds, and history reads.
4. Build with the existing Dioxus components; these HTML files are design artifacts only.
