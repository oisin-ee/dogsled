# Session-bound live activity protocol

Status: revised on 6 November 2026 against `origin/main` `31bf5a2af5b998f51589172ece96db57484e0101`. This is an implementation contract, not runtime proof. Detailed activity is Pi-only; other harnesses keep coarse status. Remote forwarding and lifecycle changes are outside scope.

## Owner and identity

`serve` owns native session identity, producer registration, ordering, freshness, and activity publication. Extend the existing detail collector (`src/service.rs:336–345`, `src/observation/details.rs`). Do not create `src/web/sessions.rs`, a portal session registry, or a portal epoch.

```text
Native key        = observation::details::Session {terminal, session, harness, transcript}; compare all four fields
Public session    = Detail::key() serialization (the existing Conversation.session / DetailSession::Available string)
Epoch             = the observation Publisher epoch owned by serve (src/observation.rs)
Producer identity = native key + serve epoch + producer generation + producer_id
```

A publication revision orders observations. It is not a session incarnation or a producer generation. When the collector observes a different binding or a confirmed disappearance, it revokes the current producer. Producer generations increase monotonically across the serve epoch and never reset when a pane is removed. This stops an old request from succeeding after an A→B→A binding transition.

Pi's session file path and header ID are producer-validation evidence, not a second identity model. The collector validates them against its current binding, reusing `src/transcript.rs` header parsing. Missing, unsaved, or unreadable evidence makes detailed activity unavailable. Main does not perform this validation today: `Session::of` only requires a non-empty native session value.

## Transport

HTTP handlers forward registration and publication commands through bounded observation IPC to the serve-owned authority. Validation and mutation happen in one authority operation; a portal-side read-check-write sequence is not acceptable. HTTP handlers perform no native reads.

Publish activity as compact values through the existing immutable publication mechanism, separate from transcript and screen bytes. Detail IPC sends the complete serialized detail payload (`src/observation/ipc.rs:337–343,430–445`), so activity must not be embedded in it; rapid activity changes would otherwise resend conversation content. Activity stays host-local, is not exported to peers, and is never used for lifecycle decisions.

## Freshness (lead decision)

Two clocks, both required for detailed display:

- **Producer lease:** the 15-second receipt lease renewed by 5-second heartbeats. A heartbeat renews producer freshness only.
- **Native evidence:** the binding that authorized the producer must come from a source sample measured within **two detail ticks (10 seconds)**. Use the source's own measurement time (`src/observation/collect.rs` `measured`), never publication time.

Panes with a registered producer are sampled for binding at the detail tick (`DETAIL_TICK`, 5 s). This sampling checks only the cheap native binding. It never starts transcript or screen capture: producer interest is not viewer interest (`src/observation/details.rs:244–325,350–413`). Stale attribution is therefore bounded at 10 seconds. It replaces the earlier two-second ceiling, which main's 5 s and 10 s sampling cannot honour.

Retain the 128-pane activity bound. Reject capacity exhaustion; never evict an active producer. Release expired producer-only interest without treating expiry as task completion.

## Wire contract

```ts
type ActiveTool = { call_id: string; name: string; parent_call_id: string | null };
type LiveState = {
  phase: "idle" | "working" | "thinking" | "responding" | "tool_running";
  active_tools: ActiveTool[];
};
type ProducerPayload =
  | { kind: "snapshot"; state: LiveState }
  | { kind: "unavailable"; reason: "resyncing" | "overflow" }
  | { kind: "closed" };
// The producer cannot compute the serve-side key, so it presents native evidence.
// serve verifies the evidence against its current binding and returns the key.
type ProducerEvidence = { session_file: string; session_id: string };
type RegisterProducer = {
  version: 1; epoch: string | null; expected_generation: number;
  producer_id: string; evidence: ProducerEvidence;
};
type PublishActivity = {
  version: 1; epoch: string; generation: number;
  producer_id: string; session: string; sequence: number; // session echoes the acked key
  payload: ProducerPayload;
};
type ActivityAck = {
  version: 1; pane: string; epoch: string; generation: number;
  producer_id: string; session: string; sequence: number;
  heartbeat_ms: 5000; ttl_ms: 15000;
};
type UnavailableReason = "unsupported_harness" | "session_unverified" | "native_unavailable"
  | "no_producer" | "no_snapshot" | "expired" | "producer_closed" | "resyncing" | "overflow";
type ActivityView = {
  version: 1; pane: string; epoch: string; revision: number;
  session: string | null; coarse_state: string | null;
  producer: { generation: number; id: string | null };
  detail:
    | { availability: "available"; sequence: number; received_at_ms: number;
        valid_for_ms: number; state: LiveState }
    | { availability: "unavailable"; reason: UnavailableReason };
};
type ActivityError = {
  error: string;
  code: "invalid_request" | "unsupported_version" | "unauthorized" | "agent_missing"
    | "session_unverified" | "session_changed" | "stale_generation" | "sequence_conflict"
    | "producer_closed" | "upstream_invalid" | "upstream_unavailable" | "capacity";
};
```

GET `/api/agents/{pane}/activity` returns `ActivityView`. POST `/api/agents/{pane}/activity/producer` registers and returns `ActivityAck`. PUT `/api/agents/{pane}/activity` publishes and returns `ActivityAck`. GET `/api/agents/{pane}/activity/events` sends complete `ActivityView` values under SSE event `activity`. Keep the existing agent event route unchanged.
Producer mutations require the existing bearer token (`guard::bearer_allowed`), even with a matching Origin; the general guard currently accepts matching-Origin mutations without one (`src/web/guard.rs:105–119,134–141`). Reuse `PortalClient`; do not add a credential store. Return no-store responses.
Reject unknown fields, unsupported versions, duplicate tool IDs, invalid counters, and invalid phase/tool combinations. Idle requires no active tools; tool-running requires at least one. Request size is 64 KiB. Limit active tools to 64, IDs to 256 bytes, names to 128 bytes, paths to 4096 bytes, session IDs to 1024 bytes, and pane IDs to 128 bytes. Malformed input is 400, failed bearer authentication 403, correlation conflicts 409, oversized bodies 413, and capacity refusal 503.
Never carry reasoning text, arguments, results, or raw event objects.

## Ordering and expiry

Registration is compare-and-swap against the current epoch, session stamp, and generation. A successful registration allocates a new generation, acknowledges sequence zero, and clears previous detail. Identical retries return the original acknowledgement without renewing freshness. Retired or conflicting registrations cannot replace the current producer.
The producer freezes its registration request. On a generation conflict, it stops that attempt rather than repeatedly taking ownership back. A later native session-start can register again. An overlapping startup can therefore fail closed into unavailable detail.
Each publication increases its sequence. Gaps are permitted because snapshots are complete. Higher sequences replace state. Identical duplicates acknowledge without renewing the lease. Lower sequences or changed content at the same sequence conflict. Closure terminalizes only its registration; a delayed old close cannot clear a replacement.
Use monotonic receipt time for the 15-second lease. Display timestamps are not ordering authority. Heartbeats arrive every five seconds. Newer snapshots can restore an expired registration if it remains current. Expiry means unavailable, never finished or idle.
SSE starts with the current snapshot and then sends latest-value replacements. Do not replay an event backlog. Browser disconnect immediately clears detail; a keepalive does not renew freshness. Remaining validity also clears detail when it expires. Server shutdown cancels streams and observation work.

## Pi reducer and publisher

Initialize idle only when `ctx.isIdle()` confirms it. Startup while busy is resyncing. Agent start is working, not inferred thinking. Track active assistant thinking/text blocks and active tools separately. Block end and message end clear their own stream state. Tool-call argument streaming is preparation, not execution. Execution events insert/remove call IDs, including parallel and nested calls.
Running tools take precedence. Otherwise show the most recently observed active stream block, then working. `agent_end` is not final settlement. `agent_settled` clears active state and supplies idle, including aborted runs; idle does not claim successful completion. Shutdown disposes locally and attempts closure.
Cap both tracking collections at 64. Overflow stays unavailable until a verified idle boundary resets tracking. Do not truncate into false emptiness.
Hooks only reduce events and schedule publication. Keep one request in flight and one replaceable pending snapshot. Coalesce publications to at most once per 250ms. Use a one-second request deadline, exact-request retries after ambiguous outcomes, and a disposed-runtime check in every callback. Telemetry failure must not reject or delay agent hooks.

## Consumer rules

Use main's session-keyed browser cache (`web/src/conversations.rs:42–83`); do not replace it. Activity must match the selected native key and the current serve epoch. Disconnect, expiry, invalid input, or unknown identity clears detailed activity only, never history or lifecycle state. Process activity separately from the agent event callback, which bumps the transcript refresh on every event (`web/src/agent_view.rs:227–248`). Heartbeats must not refresh history.

## Delivery sequence

1. **Serve-owned activity authority.** Paths:
   - `src/observation/live_activity.rs` (new)
   - `src/observation/tests/live_activity.rs` (new)
   - `src/web/activity.rs` (new)
   - `src/observation.rs`, `src/observation/details.rs`, `src/observation/ipc.rs`, `src/service.rs`, `src/transcript.rs`, `src/web/mod.rs`, `portal-types/src/agents.rs`

   Acceptance:
   - `cargo test --bin dogsled observation::`, `cargo test --bin dogsled transcript::`, `cargo test --bin dogsled web::`, and `cargo test -p portal-types` pass.
   - The tests cover replacement and A→B→A races, duplicate and conflicting sequences, delayed closure, expiry, restart, bearer enforcement, producer-only interest, and capacity exhaustion.
   - Extra viewers and heartbeats do not increase native reads or transcript reads.
2. **Pi producer and browser activity data.** Paths:
   - `pi/activity.ts` (new) and its test, `pi/index.ts`
   - `web/src/activity.rs` (new): a data hook and state only, with no rendering
   - `web/src/api.rs`

   Acceptance:
   - Node tests and the typecheck pass.
   - `cargo test -p web`, wasm Clippy, and fmt pass.
   - The tests cover parallel tools, interrupted settlement, overflow, reload and shutdown races, offline nonblocking hooks, session mismatch, epoch replacement, and disconnect clearing.
3. **Timeline rendering** (`timeline.md`) consumes history and activity. It owns `agent_view.rs`, `conversations.rs`, CSS, and component tests.

Known gaps on main, outside these units unless a unit's tests require them: `herdr_socket::Client::agent_list` turns a malformed list into an empty one (`src/herdr_socket.rs:148–151`), and `activity::newline_before` suppresses I/O errors.

## Primary evidence

The project pins Pi 1.3.0. Verified references: [SessionManager](https://github.com/earendil-works/pi/blob/71023aec1546d6d677a65669c5665c0927ce0dd0/packages/coding-agent/src/core/session-manager.ts), [extension events](https://github.com/earendil-works/pi/blob/71023aec1546d6d677a65669c5665c0927ce0dd0/packages/coding-agent/src/core/extensions/types.ts), and [assistant stream events](https://github.com/earendil-works/pi/blob/71023aec1546d6d677a65669c5665c0927ce0dd0/packages/ai/src/types.ts).
