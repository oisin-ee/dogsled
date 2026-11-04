# Conversation view

Status: the owner selected **Message timeline** through Impeccable on 4 November 2026. Build path: code-led.

The owner rejected the first text-only question on 4 November 2026. The owner requested visual options from Impeccable's tools.

## Scope

Improve the existing portal conversation, not the surrounding application. Preserve the dark console, official controls, persistent inbox, agents, todos, and pinned composer.

The conversation must identify who spoke, when the message occurred, and where the message came from. Live activity must remain visible while the owner reads history.

## Verified causes

Source revision: `a0ce8764258c4a70d968a53522a4e6f1b9c5ef6c`.

- `web/assets/console.css:448–481` centers each agent reply separately. Owner messages align to the full panel, with an 80% maximum width. The composer also spans the panel.
- `web/src/agent_view.rs:630–646` renders anonymous articles without visible message metadata. Its activity indicator is only `Working…`.
- `portal-types/src/agents.rs:55–68` stores only message role and text.
- `src/activity.rs:211–267,402–444` discards message metadata and excludes Pi custom deliveries.
- `pi/inbox.ts:145–150` already records sender, kind, creation time, and message ID. The conversation parser does not preserve them.

The layout detector reported no findings for `web/src/agent_view.rs` and `web/assets/console.css`. This result does not prove usable hierarchy.

## Visual options

Impeccable surface round: Operate, existing visual world, code-led.
Seed reference: `seed:7476680800fd7d59`.

The grounded candidate list is centered chat, a compact author rail, grouped exchanges, chat with separate activity, an inline execution stream, a time-gutter timeline, and request chapters.

The dealt structures were 7, 6, and 1. The visible options use these structures:

| Option | Structure | Trade-off |
| --- | --- | --- |
| Turn by turn | Request chapters with nested worker reports and optional earlier exchanges | Clear request context, but long exchanges need more scrolling |
| Message timeline | Chronological messages, a time gutter, threaded source labels, and compact activity summaries | Strong provenance, but more operational detail |
| Focused chat | Compact owner bubbles and open agent replies inside one centered column | Familiar, but retains slight left-right alternation |

The HTML sketches are design artifacts, not application code:

- `.impeccable/mocks/decision/chat-conversation/turns.html`
- `.impeccable/mocks/decision/chat-conversation/timeline.html`
- `.impeccable/mocks/decision/chat-conversation/focused.html`
- `.impeccable/mocks/decision/chat-conversation/options.json`

Each sketch shows a 1440×900 first viewport with illustrative content. The decision page supports expansion, selection, and another round.

`chat-conversation.squig.json` now records the selected timeline at 2560px, 1440px, and 1024px. Squig reported no text overflow or missing glyphs. The lead inspected its rendered desktop frame.

## Common layout requirements

The history, activity, and composer share one centered reading column. The current sketches use a 640px maximum column and 24px side insets.

Every author shares one prose column. A 46px time gutter and 12px timeline track sit beside it. Sender, source, and time remain visible without hover.

Owner messages use a quiet inset surface. Agent replies remain open prose. Worker reports carry explicit attribution. Tool summaries use compact disclosure rows.

Live activity sits above the pinned composer, outside the history scroll area. Message arrivals do not pull the owner away from older messages.

The implementation must preserve Markdown rendering, code and table overflow, image attachments, keyboard focus, and pane-switch history.

## Data findings

An independent architect inspected the specified revision without changing source. The final report identifies these requirements:

1. Preserve native message IDs, source metadata, and valid timestamps in the transcript adapter.
2. Show Dogsled deliveries as attributed context, not as owner messages. Do not recover sender identity from prose.
3. Preserve transcript order. Missing times remain unknown rather than becoming browser time or file modification time.
4. Drive detailed Pi activity from live message and tool events. Coarse working state alone does not prove thinking.
5. Keep display activity separate from lifecycle completion and cleanup.

The proposed event path uses Pi message-block events, tool-execution events, and settlement. It sends metadata only, never reasoning text or tool arguments.

Detailed activity starts with the verified Pi event interface. Other harnesses retain honest coarse states until their integration supplies equivalent evidence.

The reviewed implementation contract is in `briefs/chat-conversation/activity-protocol.md`. It defines native session stamps, authenticated producer registration, ordered full snapshots, bounded freshness, and unavailable states.

Session-safe reads are a separate prerequisite. This fixes false 404/empty responses and prevents cached history from acquiring another session's attribution. It does not add a second history model, select an active transcript branch, or change lifecycle.

Relevant primary evidence from that report:

- [Pi session format](https://github.com/earendil-works/pi/blob/71023aec1546d6d677a65669c5665c0927ce0dd0/packages/coding-agent/docs/session-format.md)
- [Pi extension events](https://github.com/earendil-works/pi/blob/71023aec1546d6d677a65669c5665c0927ce0dd0/packages/coding-agent/src/core/extensions/types.ts#L1058-L1128)
- [Pi stream block events](https://github.com/earendil-works/pi/blob/71023aec1546d6d677a65669c5665c0927ce0dd0/packages/ai/src/types.ts#L798-L822)

## Acceptance after implementation

Browser evidence must cover the selected composition at narrow, normal, and ultrawide desktop sizes. Include long authors, code, tables, unavailable timestamps, and interrupted streams.

Behavior tests must cover attributed deliveries, timestamp preservation, state transitions, and retained history. No indicator can claim that work finished because telemetry stopped.

## Direction contract

THESIS: One chronological timeline makes the speaker, source, time, and current work legible. It replaces disconnected bubbles and anonymous prose.

OWN-WORLD: Preserve the existing graphite console, Inter UI text, muted metadata, 1px rules, official Dioxus controls, and semantic state colors.

STORY: The owner returns, reads the latest exchange, distinguishes worker reports from the lead, sees current activity, and sends a message.

FIRST VIEWPORT: Keep the surrounding columns. Center a maximum 640px timeline with a time gutter, author labels, quiet owner surfaces, and collapsed tool summaries. Pin the activity row and composer below it. Only history scrolls. At narrower widths, metadata wraps without hiding the author or message.

FORM: Message timeline, grounded candidate 6, selected on the Impeccable decision page. `seed:7476680800fd7d59`. The approved composition is `timeline.html`.

FINISH: unreviewed and undocumented is unfinished; this build ends with the finish review, the verdict, DESIGN.md, and every shipping raster carrying its provenance

## Delivery order

1. Preserve historical message identity, timestamps, source attribution, and bounded tool summaries in the shared conversation contract.
2. Bind conversation and metadata reads to verified native sessions. Propagate actual read failures and reject stale cache responses.
3. Add the transient activity endpoint with authenticated input, ordered snapshots, and explicit unavailable states.
4. Publish Pi activity from actual message and tool events without blocking agent execution.
5. Render the selected timeline from those contracts, then verify the integrated result in the browser.

Each writer receives one bounded unit, an exact base revision, owned paths, and focused acceptance commands. Shared-file changes run in dependency order.

The history unit is under implementation on `feat/conversation-timeline`. Its focused checks pass, but acceptance still requires a committed result and review. No release, installation, or fleet rollout is authorized by this plan.
