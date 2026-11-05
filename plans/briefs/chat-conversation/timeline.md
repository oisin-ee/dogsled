# Render the selected conversation timeline

Status: draft. Dispatch only after the history and live-activity contracts are accepted. The dispatch supplies their exact revisions and final wire types.

## Outcome and cause

Render the owner's selected Message timeline inside the existing Dioxus console.
`web/assets/console.css:450–490` centers assistant text but anchors owner bubbles and the composer against the full panel. `web/src/agent_view.rs:630–646` renders anonymous role/text articles with array-index keys and places Working inside scrolling history. The fix belongs in this composition and its data consumers.

## Visual authority

Read `plans/chat-conversation.md`, `.impeccable/surfaces/web-src-agent-view-rs.md`, `DESIGN.md`, and the approved `.impeccable/mocks/decision/chat-conversation/timeline.html`. The HTML controls composition, not its illustrative messages or states. `plans/chat-conversation.squig.json` records the selected layout at 2560, 1440, and 1024px.
Preserve the existing graphite palette, Inter text, official controls, inbox, agents, todos, waiting requests, and viewport-contained scrolling. Do not hold a new design round.

## History wire contract (reviewed, staged; exact commit supplied at dispatch)

The history unit defines these shapes in `portal-types/src/agents.rs`. Render from these types; do not reparse prose.

- `Conversation { pane, turns: Vec<Turn>, truncated: bool }`. The session-boundary unit adds nullable `session: SessionStamp`. When `truncated` is true, older history exists but was not returned; show a quiet notice at the top of history, never a fake complete start.
- `Turn { id, role, text, source, timestamp: Option<Timestamp>, tool: Option<ToolSummary> }`. Use `id` as the key. It is the native ID where available, otherwise `{harness}:offset:{n}`; tool rows add `:tool:{call_id}`.
- `Role`: `user`, `assistant`, `context`, `tool`. `context` covers displayed extension and Dogsled messages; it is never the owner.
- `Source` (tagged `kind`):
  - `harness {harness, model?, provider?}`. `model` and `provider` appear only on assistant records that recorded them.
  - `custom {custom_type}`. This is visible extension context, including Dogsled gate messages without delivery records. It has no sender; label it by type, never as You or as a named worker.
  - `dogsled {sender, delivery_kind}`. This is an attributed inbox delivery; label it with `sender`.
- `Timestamp { ms, meaning }`. `meaning` is one of:
  - `message`: the harness message time. For Pi assistant records this is the request start, not response completion.
  - `recorded`: when the transcript entry was written.
  - `created`: when the Dogsled delivery was created.

  Expose the meaning in the accessible full date/time where it differs from "sent".
- `ToolSummary { call_id, name?, status }`. `status` is one of:
  - `completed`: a result was recorded. This does not mean the payload reports success.
  - `error`: the recorded result was an error.
  - `unknown`: no matching result was read. This means "no recorded outcome", never "running now".

  Tool rows carry no arguments, output, or reasoning.

## Composition

- Give history, live activity, and composer one centered column with a 640px maximum and 24px side insets where space permits. The timeline has a 46px time gutter, 12px marker track, and flexible prose. Do not recreate the separated right-aligned owner column.
- Put actual author/source labels above messages. Distinguish native user input, the selected assistant, named Dogsled deliveries, and other visible context. Never label a worker delivery You. Show recorded per-message model metadata only where present; current agent metadata is not historical provenance.
- Render real timestamps with a semantic time element and an accessible full date/time. Expose timestamp meaning when needed. Missing time has a quiet dash and accessible Time unavailable text. Use platform date formatting rather than a handwritten calendar. Wrap long metadata without hiding content.
- Give owner messages quiet inset surfaces and keep assistant prose open. Use stable record IDs for keys. Preserve the existing Markdown renderer and horizontal scrolling for code and tables within the content column.
- Group adjacent tool records into compact disclosures using the existing official Collapsible or Accordion components. List actual names and recorded outcomes; do not present historical unknown completion as current execution. Keep raw arguments, output, and reasoning out of this view.
- Keep the current activity row immediately above the composer, outside history scrolling. Use the accepted snapshot contract; detailed phases come only from fresh session-matching telemetry. Blocked, disconnected, unsupported, and idle states must remain distinct. Announce meaningful phase changes politely, not every heartbeat.

## Data and behavior

History must load independently of agent metadata and activity. Preserve the bounded shell-owned history cache. Use the accepted session identity and availability contract so transient failures retain history and a genuinely different session cannot inherit another conversation. Do not weaken the backend's 404 distinction.
Keep activity updates separate from transcript refreshes: token and tool updates must not repeatedly parse Markdown or refetch the full transcript. Continue to refresh history when a new durable message can exist and through the existing bounded fallback.
Retain follow-at-bottom behavior and the 48px threshold. An owner reading older messages must not jump when messages, activity, or tool results arrive. Preserve Enter/Shift+Enter, IME input, attachment upload/removal, focus, draft retention, pending reviews/questions, and interruption controls.

## Scope

Expected owned paths: `web/src/agent_view.rs`, `web/src/api.rs`, `web/src/conversations.rs`, `web/src/component_tests.rs`, `web/assets/console.css`; a focused module under `web/src/` is permitted only if it owns a real conversation boundary. Do not edit generated official components or third-party code. Ask before changing dependencies or unrelated panels.

## Focused evidence

Run the accepted web tests, wasm-target Clippy, and format checks. Cover source labels, missing metadata, timestamp semantics, stable keys, tool disclosures, activity priority, telemetry loss, session mismatch, and history retention. Test markup through Dioxus SSR where possible; browser formatting must remain a platform boundary rather than a fabricated test implementation.
The lead owns bundle generation and the final repository gate. The writer does not build bundles, start servers or browsers, install packages, or deploy.

## Browser acceptance owned by the lead

Inspect 1024×900, 1440×900, 2560×900, the original user viewport if different, and 390px mobile. Exercise long names, multiline messages, long code/tables, unknown times, loading, empty, interrupted telemetry, blocked prompts, expanded tools, image attachments, keyboard focus, scrolled-up history, and pane switching. The document must not scroll; the composer and activity row must remain visible. Capture the final revision and perform the independent Impeccable finish review.

## Delivery

Commit with hooks enabled. Push only the assigned branch. No PR, merge, release, installation, or fleet rollout. Return the exact revision, changed paths, focused command results, any remaining behavior gap, and source/formatting decisions needed for review.
