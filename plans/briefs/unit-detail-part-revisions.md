# U1: per-part detail revisions in agent events

Plan: `plans/git-files-sidebar.md`, unit U1. Base: `a6e3f8a` (code is identical at later docs-only commits).

## Outcome

Each agent event carries a separate conversation revision and screen revision. The local event stream sends an event only when the event content changes.

## Cause

- `src/observation/details.rs:187` sets `conversation_revision` to the whole detail revision, so a screen-only change looks like a conversation change.
- `src/observation/ipc.rs:621-623` (`write_detail`) puts `publication.revision` (the global portal publication) in each detail frame. The detail `Reference` never reaches the frame.
- `src/web/agents.rs:711` dedupes events on `(payload, connection.publication.revision)`. As a result, every portal publication sends an agent event, and the browser reloads the whole view.

## Edits

1. `portal-types/src/agents.rs`: add `pub struct DetailRevisions { pub conversation: u64, pub screen: u64 }` (serde, `deny_unknown_fields`). Add `#[serde(default, skip_serializing_if = "Option::is_none")] pub revisions: Option<DetailRevisions>` to `AgentEvent` (line 403). `None` means a peer that does not send revisions.
2. `src/observation/details.rs`:
   - In `Reference` (lines 138-147), replace `conversation_revision` with `parts: DetailRevisions`. Keep `screen_revision`, the native Herdr revision, which is tested at `tests/details.rs:187`.
   - Give `Payload::new` a `previous: Option<&Payload>` argument. A part keeps the previous part revision when the session and that part's `Content` are both equal. Otherwise it takes the new detail revision.
   - Pass the current `output` value at all three call sites (lines 477, 498, 580-585).
   - Add `#[serde(default)] pub parts: Option<DetailRevisions>` to `DetailFrame` (line 150).
3. `src/observation/ipc.rs`: `write_detail` takes `parts: Option<&DetailRevisions>` and writes it as `"parts"` in the header. The detail loop passes `Some(&payload.reference.parts)`; the missing-agent frame (line 382) passes `None`.
4. `src/web/agents.rs`:
   - `agent_event` (line 739) sets `revisions` from `publication.parts`.
   - `events` (lines 705-716) dedupes on the payload string alone. Drop the publication revision from the signature.
   - `peer_events` is out of scope; U2 replaces it.

## Paths

Owns: `portal-types/src/agents.rs`, `src/observation/details.rs`, `src/observation/tests/details.rs`, `src/observation/ipc.rs`, `src/web/agents.rs`.
Reads: `src/observation/`, `src/web/`, `web/src/api.rs`, `web/src/agent_view.rs`. Do not change `web/`; U4 owns the client.

## Acceptance

Add tests that prove each of these:
- In `src/observation/tests/details.rs`, a screen-only change advances `parts.screen` and leaves `parts.conversation` unchanged. A transcript change advances `parts.conversation`. A session replacement advances both.
- In `src/web/agents.rs` tests, two portal publications with an unchanged detail send one `agent` SSE event. A screen change sends a second event, and its `revisions.screen` is greater.
- In `portal-types`, an `AgentEvent` without `revisions` still deserializes.

Then run `cargo test --bin dogsled observation::tests::details web::agents` and `cargo test -p portal-types`. All tests must pass.

## Checks

`moon run dogsled:fmt-check`, `cargo clippy --all-targets -- -D warnings`, and the tests above.
