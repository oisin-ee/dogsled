---
version: 1
slug: "web-src-agent-view-rs"
primary_target: "web/src/agent_view.rs"
related_targets: ["web/assets/console.css","web/src/conversations.rs"]
---

# Conversation timeline

Mode: Operate. Scope: the conversation inside the existing portal. Preserve the inbox, agents, todos, palette, official components, and viewport behavior from the portal surface.

The owner selected Message timeline through Impeccable on 4 November 2026. Build path: code-led. Approved sketch: `.impeccable/mocks/decision/chat-conversation/timeline.html`. Editable layouts: `plans/chat-conversation.squig.json`.

## Direction contract

THESIS: One chronological timeline makes the speaker, source, time, and current work legible. It replaces disconnected bubbles and anonymous prose.

OWN-WORLD: Existing graphite console, Inter text, muted metadata, 1px rules, official Dioxus controls, and semantic state colors. No new palette or decoration.

STORY: The owner returns, reads the latest exchange, distinguishes worker reports from the lead, sees current activity, and sends a message.

FIRST VIEWPORT: Preserve the surrounding columns. Center a maximum 640px timeline with a 46px time gutter, a 12px marker track, author labels, quiet owner surfaces, and collapsed tool summaries. Pin the activity row and composer below it. Only history scrolls. Metadata wraps at narrower widths without hiding the author or message.

FORM: Message timeline, grounded candidate 6, selected from three rendered options. `seed:7476680800fd7d59`. Signature: messages and compact tool events share a time axis; current activity stays visible while history scrolls.

FINISH: unreviewed and undocumented is unfinished; this build ends with the finish review, the verdict, DESIGN.md, and every shipping raster carrying its provenance

## State requirements

Use transcript metadata for time and source. Missing metadata stays unknown. Derive detailed activity only from actual agent events. Telemetry loss never means completed work. Keep Markdown, attachments, keyboard access, and pane-switch history.
