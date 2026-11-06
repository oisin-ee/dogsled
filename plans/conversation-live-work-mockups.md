# Conversation live-work mockups

## Scope and status

Status: three new layout directions for owner review. No production behavior, producer, or data contract changed.

On 2026-11-06 the owner chose:

1. **Option B, live reasoning stream.** The agent's reasoning text streams in real time.
2. **Tool calls show their arguments the way Pi and Herdr show them now.** No separate allowlist or redaction layer.
3. **The conversation needs more space.**

The owner rejected the earlier mockups in `.impeccable/mocks/conversation-live-work/`: an inline block plus a dock, drawn in the old timeline look, with invented content.
They are superseded and are not a starting point. The new directions are in `.impeccable/mocks/live-reasoning/`.

## What the previews show

Every direction plays back the same real Pi turn, from `~/.pi/agent/sessions/--Users-oisin-dev-dogsled--/2026-11-06T11-53-15-658Z_*.jsonl`:

- The owner asked "why is this happening?" with a screenshot at 11:53:45. The claude-opus-5-5 lead worked for 2m 31s (6 thinking blocks, 13 tool calls) and replied at 11:56:16.
- The owner wrote "no, give me proper responses, not some handwavey shit." at 12:26:00.
- The previews freeze at 12:27:19, mid-thought, after 8 thinking blocks and 8 tool calls of the second turn. That last thought types in at the live edge.

The text is the recorded text: the thinking blocks, the exact bash commands, the output tails, timestamps, and durations from tool-result timestamps.
Only the agent list, host meters, and todo column are illustrative.
Thinking renders italic and muted, as Pi's TUI renders it. The newest paragraph is full contrast.
Bash calls render as `$ command` with the last lines of output and an "N earlier lines" fold, as Pi does.
A finished turn folds its work into one line, `Worked for 2m 31s · 6 thoughts · 13 tool calls`, above the reply. Opening it shows the full recorded work.

## The three directions

| File | Direction | Main trade-off |
| --- | --- | --- |
| `one-stream.html` | One stream, like Pi. Messages, thinking, tool calls, and replies in recorded order. Inbox and agents share a 300px left column; todos sit on the right from 1400px. | Long reasoning pushes earlier replies up; the fold line is the way back to them. |
| `split.html` | Dialogue beside Work. The left pane has only messages and replies. The right pane streams thinking and tool calls, with the current todo in its header. Below 1100px they become two tabs, and the composer stays pinned in both. | Two scroll regions. At narrow widths, the work is one tab away. |
| `focus.html` | Focus on one agent. Working agents become a card row across the top, each with its newest thought or command. The inbox keeps its own column. The chosen agent's stream fills the width. | With many agents the card row scrolls sideways; the full list is behind "All agents". |

All three keep the inbox visible and the composer pinned to the bottom. The agents list shows each agent's live phase and its newest thought or command.

Captures (each `.png` has a `.png.json` provenance sidecar):

- `<direction>-1600.png`, `<direction>-1280.png`, `<direction>-900.png`: live edge at 1600 × 1000, 1280 × 800, and 900 × 800.
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
- The generator read the session JSONL and wrote the three HTML files. It is a one-off design script and is not in the repository.
- The managed browser rendered each direction at 1600 × 1000, 1280 × 800, and 900 × 800. No scroll region, header, or top bar had horizontal overflow at any size.
- The Impeccable detector found:
  - `overused-font` (Inter) on all three. Inter is the `DESIGN.md` font.
  - `flat-type-hierarchy` on `one-stream.html` and a `cramped-padding` note on the split tabs. These match the portal's 13–14px Operate scale and the `segmented-tabs` token (2px padding).
  - It found no other issues.

## Before implementation

1. The owner picks a direction, or rejects all three with a reason.
2. Update the editable Squig layout (`plans/chat-conversation.squig.json`) to the chosen direction.
3. Define the protocol change: reasoning and tool-argument fields, bounds, and history reads.
4. Build with the existing Dioxus components; these HTML files are design artifacts only.
