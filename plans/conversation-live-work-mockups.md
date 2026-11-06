# Conversation live-work mockups

## Scope and status

Status: revised mockups for owner review. No production behavior, producer, or data contract changed.
The owner has not approved the rendered mockups for implementation.

The owner chose this structure earlier: an inline live-work block in the timeline, and a compact dock when the live turn leaves the viewport.
On 2026-11-06 the owner chose what goes inside it:

1. **Option B, live reasoning stream.** The agent's reasoning text streams in real time.
2. **Tool calls show their arguments the way Pi and Herdr show them now.** No separate allowlist or redaction layer.
3. **The conversation needs more space.** The layout must give it more width.

This replaces the earlier revision, which kept reasoning private and showed only todo titles, visible assistant text, and allowlisted tool targets.

## Why the current view looks idle

- `pi/activity.ts:393-404` turns every `thinking_delta` into a bare `thinking` phase and drops the text.
- `src/activity.rs:142,196` and `portal-types/src/agents.rs:131,245` drop reasoning and tool arguments from history and live state.
- `web/src/agent_view.rs:764-800` folds tool records into one row of names, such as `5 tool records · bash, bash, read`.
- `web/src/agent_view.rs:914-945` shows the live state as one label above the composer.

An agent chose that privacy boundary during the timeline work. Memory does not record an owner request for it.

The data exists. A scan of recent Pi transcripts in this repository on 2026-11-06 found:

| Model | Steps with reasoning text | Shape |
| --- | --- | --- |
| gpt-6-astra | 55 of 56 | Short bold headlines, such as `**Inspecting sidebar tabs**` (median 57 characters) |
| claude-opus-5-5 | 38 of 39 | Prose (median 579 characters, longest 2,788) |

Visible assistant text is rare: one session had 9 text blocks against 56 reasoning blocks.
So reasoning is the main signal that the agent is working, and the earlier "agent update" design would be empty most of the time.

## Preview files

All files are in `.impeccable/mocks/conversation-live-work/`. Content is illustrative.

| File | View | Capture |
| --- | --- | --- |
| `inline.html` | Claude lead thinking: earlier thoughts folded, Pi-style tool calls, live reasoning at the edge | `inline.png`, 1600 × 1000 |
| `dock.html` | Reading an earlier finished turn; the dock streams the newest reasoning; one inbox item waits | `dock.png`, 1600 × 1000 |
| `narrow.html` | GPT worker running `cargo test`, headline-style reasoning, live output tail | `narrow.png`, 900 × 800 |

`review.json` presents the three views through Impeccable:

```sh
/Users/oisin/.pi/agent/skills/impeccable/scripts/impeccable serve-question --start --payload .impeccable/mocks/conversation-live-work/review.json
```

The HTML files work without that server. Native disclosures expand thoughts and finished work.
Controls are disabled; the previews do not send messages or interrupt agents.

## Design

### Live turn

- The turn header shows the author, model, harness, the current phase, and the turn's elapsed time.
- The work reads top to bottom as one stream, in the order Pi recorded it:
  - **Finished thought:** one line, `Thought · 4s` plus its first line. Click to expand the full text.
    Headline-only reasoning (typical for GPT models) shows the headline with no disclosure.
  - **Tool call:** a status icon, the call as Pi renders it, and the duration.
    `$ command` for bash, `read path:lines`, `edit path` with diff lines, the tool name and arguments otherwise.
    Output shows a short preview with "N more lines", as Pi does. Errors show the exit status.
  - **Nested calls:** calls with a parent call ID (for example, inside `codemode`) indent under their parent.
  - **Live thought:** a `Thinking · 18s` label and the streaming text with a caret. Older sentences are muted; the newest paragraph is full contrast.
- A footer states the stream source and the age of the last event, and holds Interrupt.
- The history follows the live edge while the owner is at the bottom. When the owner scrolls up, it stops following.

### Dock

The dock appears only when the live turn is out of view. It shows the phase, elapsed time, author, and the newest two lines of reasoning, or the running command.
It has Jump to live and Interrupt. It sits above the composer and never covers history.

### Finished turns

A finished turn keeps its whole work stream behind one line: `Worked for 3m 40s · 2 thoughts · 5 tool calls`.
The reply text stays visible below it. Nothing is discarded when a turn ends.

### Agents list

Each agent row gets a second line with its live phase and elapsed time or current command, such as `Thinking · 1m 12s` or `Running · $ cargo test -p dogsled-web`.
This shows which agents are moving without opening each one.

### Space

Proposal: the inbox and agents share one 280px left column. The inbox sits on top (up to 44% of the height, its own scroll), and agents fill the rest.
This frees 248px for the conversation at every width. At 1600px the conversation column grows from 772px to 1020px.
The thread measure grows from 640px to 920px. Prose and reasoning paragraphs keep a 72-76ch measure; tool calls and output use the full width.
Below 1400px, todos fold into a tab as before. The agents list no longer needs to fold into a tab below 1100px.

This changes the 2026-11-02 steer of "two left sidebars". The inbox still never folds or hides. The owner must confirm this change.
Alternatives if the owner rejects it: a collapsible agents rail, or narrower todos (about 60px gain).

### Liveness

One pulsing marker per view, and only while fresh telemetry arrives inside its validity window.
On a telemetry gap the marker stops, turns muted, and the footer names the gap. A gap never reads as finished.
The Impeccable detector flags the pulse; it is kept because it is tied to live data. Reduced-motion preferences stop it.

## Data the implementation needs

| Display | Source | Current gap |
| --- | --- | --- |
| Live reasoning text | Pi `message_update` `thinking_delta` events | Publisher drops `delta`; `LiveState` has no text field and denies unknown fields |
| Finished thoughts in history | Transcript `thinking` blocks | Transcript reader drops them |
| Tool call line | Transcript `toolCall` arguments; Pi `tool_execution_start` args | Not read or published |
| Tool output preview and status | Transcript tool results; Pi `tool_execution_end` and any partial-output events (unverified) | Only completed/error/unknown is kept |
| Nested calls | `parentToolCallId` | Live state has it; history does not |
| Durations | Event timestamps | Pi assistant timestamps mark request start, not completion |

Live frames are latest-value only. A viewer that connects mid-turn must get the turn's earlier thoughts from the transcript, not from the stream.
Reasoning and output need size bounds per block and per frame. The bounds are an implementation decision.
Claude and Codex harnesses keep the coarse "Working" state until a producer exists for them.

Pi renders tool calls through each tool's `renderCall` (see Pi's `export-html/template.js` and `tool-renderer.js`).
The portal is Rust, so it cannot call those renderers. It must reproduce the built-in formats and fall back to tool name plus arguments for other tools.

## Not mocked

- Telemetry gap and reconnect states (rules above).
- An interrupted turn.
- Codex reasoning summaries.

## Method and evidence

Impeccable context was loaded for `web/src/agent_view.rs`. The previews follow `DESIGN.md` and the conversation surface brief.
The managed browser rendered all three views at their capture sizes. The document had no overflow or document scrolling, and the history had no horizontal overflow.
Captures paused animations so the caret and marker show. Sidecar JSON files record provenance; no image model made them.
The Impeccable detector ran on all three files. Contrast, heading-order, and header-padding findings were fixed. The pulse finding remains by design.
The old `.prompt.txt` files recorded failed image-generation requests for the previous revision. They were removed; Git history keeps them.

## Before implementation

1. The owner reviews the three views and confirms or rejects the shared left column.
2. Update the editable Squig layout (`plans/chat-conversation.squig.json`) for the new column layout.
3. Define the protocol change: reasoning and tool-argument fields, bounds, and history reads.
4. Build with the existing Dioxus components; these HTML files are design artifacts only.
