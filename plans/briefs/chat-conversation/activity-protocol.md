# Session-bound live activity protocol

Status: implementation contract reviewed against Dogsled source. This is not runtime proof. Detailed activity is Pi-only; other harnesses keep coarse status. Remote forwarding and lifecycle changes are outside scope.

## Owners and identity

An `AppState`-owned session registry observes native bindings. A transient activity owner uses those stamps for producer ordering, freshness, and subscribers. Neither owner writes lifecycle or ledger state.

```ts
type SessionKey = {
  harness: string;
  reference: { kind: "path" | "id"; value: string };
  id: string | null;
};
type SessionStamp = { server_epoch: string; revision: number; key: SessionKey };
```

Create one random epoch per portal process. Session revisions increase within that epoch when a native binding changes or disappears and returns. Counters are JSON-safe integers; overflow refuses.
For Pi, require an explicit native path and the header ID read from that transcript. The publisher must match both Pi getters. Do not infer identity from filenames, modification times, or titles. An unsaved or unreadable Pi session remains unverified. Other harnesses preserve their native reference; an ID reference supplies `id`, while a path reference has `id: null`.
Use one serialized native-observation path for metadata, conversation, and activity. Do not hold a data lock over socket or file I/O. Discard obsolete observations. Track at most 128 panes; never evict a live producer to admit another. Confirmed disappearance releases records; revisions and producer generations never reuse values within an epoch.
While activity is consumed or published, reconcile tracked panes once per second. An observation older than two seconds cannot authorize detailed publication or display. These limits bound stale attribution; they do not decide lifecycle completion.

## Conversation boundary

Add nullable `session` stamps to `AgentView` and `Conversation`. Keep the history writer's `Turn` and `truncated` fields unchanged.
Only a valid agent-list response can establish absence and return 404. Preserve the current 502 policy for malformed or failed upstream socket calls. Use 503 for a missing socket, unresolved native binding, or unreadable transcript. This avoids an unrelated rewrite of generic socket error taxonomy.
Propagate errors from the actual bounded transcript read, including seek/read failures. A preliminary open check is insufficient. Read the Pi header and rows through the same opened transcript, then revalidate its native binding before returning. A supported successful conversation read has a verified stamp. A successful empty conversation means an actual readable transcript contained no eligible records.

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
type RegisterProducer = {
  version: 1; server_epoch: string; expected_generation: number;
  producer_id: string; session: SessionStamp;
};
type PublishActivity = {
  version: 1; server_epoch: string; generation: number;
  producer_id: string; session: SessionStamp; sequence: number;
  payload: ProducerPayload;
};
type ActivityAck = {
  version: 1; pane: string; server_epoch: string; generation: number;
  producer_id: string; session: SessionStamp; sequence: number;
  heartbeat_ms: 5000; ttl_ms: 15000;
};
type UnavailableReason = "unsupported_harness" | "session_unverified" | "native_unavailable"
  | "no_producer" | "no_snapshot" | "expired" | "producer_closed" | "resyncing" | "overflow";
type ActivityView = {
  version: 1; pane: string; server_epoch: string; revision: number;
  session: SessionStamp | null; coarse_state: string | null;
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
Producer mutations require the existing bearer token, even with a matching Origin. Reuse `PortalClient`; do not add a credential store. Return no-store responses.
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

## Consumer races

Either metadata or conversation can establish a session stamp independently. Apply a newly verified different native key before rendering its metadata or text. Within an epoch, reject older session revisions. Invalidate old history and request generations on replacement; reject late prior-session responses.
Across server epochs, discard detail. Retained text can remain stored, but display it only after verifying the same native key. Unverified retained rows cannot acquire a new agent's attribution. Cache equality includes `session` and `truncated`. A 502/503 retains history; confirmed 404 invalidates it.

## Acceptance and delivery sequence

1. Session-safe reads: malformed list, actual I/O failures, confirmed absence, valid empty history, unsaved/custom-ID Pi sessions, session replacement, and cache races.
2. Activity API: authentication, CAS replacement, delayed requests, duplicate/conflicting sequences, expiry, restart epoch, initial/reconnected SSE, and capacity limits.
3. Pi producer: real event fixtures, parallel/nested tools, interrupted settlement, reload/shutdown races, offline nonblocking behavior, coalescing, and overflow.
4. Timeline consumer: source/time rendering, available/empty/unavailable distinctions, disconnect clearing, session exclusion, independent loading, and approved layout behavior.

## Primary evidence

The project pins Pi 1.3.0. Verified references: [SessionManager](https://github.com/earendil-works/pi/blob/71023aec1546d6d677a65669c5665c0927ce0dd0/packages/coding-agent/src/core/session-manager.ts), [extension events](https://github.com/earendil-works/pi/blob/71023aec1546d6d677a65669c5665c0927ce0dd0/packages/coding-agent/src/core/extensions/types.ts), and [assistant stream events](https://github.com/earendil-works/pi/blob/71023aec1546d6d677a65669c5665c0927ce0dd0/packages/ai/src/types.ts).
