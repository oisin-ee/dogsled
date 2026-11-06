# U8: An open agent reads each part once and keeps one activity stream

## Outcome
- Opening an agent reads each part (conversation, files, and screen when blocked) at most once per real content change.
- The activity stream stays open across conversation changes. It reconnects only when the detail session changes, or after a stream failure.

## Cause
The lead measured this with a HAR of one agent open on the U7a build (`f47c990`), at 10 s, with no edits:
`activity` ×3, `activity/events` ×3 (one cancelled), `conversation` ×2, `files` ×2.

1. **Activity restarts on every conversation fetch.**
   - `web/src/conversations.rs` `use_conversation` calls `session.set(Some(DetailSession::Available(key)))` after every accepted response, even when the key is unchanged.
   - A Dioxus `Signal::set` always notifies. `web/src/activity.rs` `use_live_activity` reads `session()`, so every conversation fetch tears down the activity fetch and stream and reopens them.
   - While an agent works, each turn restarts the activity stream. The HAR shows a restart at the moment the second conversation response lands.
2. **Activity starts before the session is known.**
   - `use_live_activity` fetches and opens a stream while `session` is `None`. `reduce` can only answer `Live::Pending` there.
   - The first event then sets the session and cancels that stream.
3. **Pending parts are fetched.**
   - When a viewer subscribes, `Registry::select` (`src/observation/details.rs`) publishes `Detail::pending`, which bumps every part revision.
   - The capture then makes conversation available (one more bump), and the Git thread does the same for files.
   - `changed_parts` (`web/src/agent_view.rs`) fetches on every revision change, so it reads conversation and files once while they are still Pending. `AgentEvent` carries no part state, so the client cannot know.

## Edits
1. **`use_conversation`:** write `session` only when the accepted key differs from `session.peek()`. A different key is a real session change and still notifies.
2. **`use_live_activity`:** while `session()` is `None`, keep `Live::Pending` and start no fetch or stream. Keep all other behaviour: epoch, expiry, the session-mismatch fence and the 1 s retry.
3. **`portal-types/src/agents.rs`:** add `AgentEvent.pending: Option<PendingParts>` with `#[serde(default, skip_serializing_if = "Option::is_none")]`. `PendingParts { conversation: bool, screen: bool, files: bool }` names each part whose content is still `Pending`.
4. **`src/web/agents.rs` `agent_event`:** fill `pending` from `publication.detail`: `Content::Pending` for conversation and screen, and `FilesState::Pending` for files.
5. **`changed_parts`:**
   - Fetch a part only when its revision changed and the part is not pending.
   - Still record the event's revisions, so the bump to available triggers exactly one read.
   - With no `pending` (older peers), keep today's behaviour.
6. **Tests:**
   - An unchanged conversation key does not notify the session signal. Extract a pure helper if needed.
   - Activity does nothing while the session is `None`. Extract a pure decision if needed.
   - `changed_parts` skips pending parts, and reads them once when they become available.
   - `agent_event` serializes `pending` from a pending detail.
   - The existing `changed_parts` tests still pass.

## Paths
- **Owns:** `web/src/conversations.rs`, `web/src/activity.rs`, `web/src/agent_view.rs`, `portal-types/src/agents.rs`, `src/web/agents.rs`, `web/src/component_tests.rs`.
- **Reads:** `src/observation/details.rs`, `web/src/api.rs`.

## Acceptance
- `cargo test -p web`, `cargo test -p portal-types` and `cargo test web::agents` pass, with the new tests.
- The lead's HAR of one agent open shows `activity` ×1, `activity/events` ×1, `conversation` ×1 and `files` ×1.
- After a later edit in the checkout, one `files` read and no activity reconnect.

## Checks
- Run `cargo fmt --all --check`, `cargo clippy --all-targets -- -D warnings` and `cargo clippy -p web -- -D warnings`.
- Commit through the pre-commit hook. If it runs long, commit detached: `setsid nohup git commit … > /tmp/u8-commit.log 2>&1 < /dev/null &`.
- The lead builds `web/public` and runs web-lint and the browser and HAR checks.

## Base
Branch `fix/accordion-width` at `f47c990`. This brief is complete; it depends on no plan file.
