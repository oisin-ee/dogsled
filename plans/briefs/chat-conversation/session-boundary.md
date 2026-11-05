# Bind conversation reads to native sessions

> **Superseded on 6 November 2026.** `origin/main` already satisfies this brief through `a469f9a`, `7e64a46`, and `262c8cf`:
> - serve-owned detail workers keyed by `observation::details::Session`;
> - `Conversation.session` and `DetailSession`;
> - `probe::agent_list` rejects malformed lists;
> - absence comes from `publication.found`;
> - `try_conversation` propagates I/O errors;
> - the browser cache is keyed by session and rejects late responses.
>
> Do not dispatch this brief. The live-activity identity is being revised onto main's `Session` key; see `activity-protocol.md`.

Status: ready after the history unit is accepted. The dispatch supplies its exact base and this document's committed revision.

## Outcome and owning causes

A verified session owns each conversation snapshot. Temporary read failures retain history; confirmed absence removes it. A reused pane cannot display another session's conversation.
`src/herdr_socket.rs::agent_list` turns malformed lists into empty lists. `src/web/agents.rs::agent_of` removes errors. The bounded reader in `src/activity.rs` hides file failures. `web/src/conversations.rs` identifies retained snapshots by pane alone.

## Contract

Read `activity-protocol.md` beside this brief, especially Owners and identity, Conversation boundary, and Consumer races. Implement those sections only. Do not add activity routes, producers, polling, or timeline styling in this unit.

- Define the shared `SessionKey` and `SessionStamp`. Add nullable `session` to `AgentView` and `Conversation`. Do not change the accepted history writer's turn representation, truncation flag, or read limits.
- Add one `AppState`-owned native-session registry in `src/web/sessions.rs`. It owns the process epoch and increasing session revisions. Serialize observations without holding the data lock across native I/O. This is transient state, not a ledger record or lifecycle authority.
- Use native references, and read Pi's header ID from the same opened file as its conversation records. Extend the existing transcript helpers rather than duplicating header parsing. Unsaved, malformed, and unreadable Pi sessions are unverified. Never choose a latest file or manufacture an identity.
- Make malformed `agent_list` responses fail. Make agent lookup preserve errors. Return 404 only after a valid response confirms absence. Preserve existing upstream 502 errors; use 503 for missing socket, unresolved identity, and unreadable transcript. Do not rewrite generic socket error taxonomy.
- Propagate actual open, seek, metadata, and read errors through the bounded conversation reader. Keep valid empty history distinct. Revalidate the native session before returning a read; do not attach a new stamp to old rows after a session switch.
- Apply stamps from metadata and conversation before their content renders. Either request may establish the stamp; neither waits for the other. Reject older revisions and late responses from an invalidated request generation. A verified different native key clears the old displayed rows. Across portal epochs, reuse retained text only after verifying the same key.
- Cache equality includes session and truncation. Keep the 16-pane bound and existing access behavior. Preserve retained rows on 502/503 without presenting them as another agent's history. Keep confirmed-404 invalidation. Do not clear a valid cache merely because an observation temporarily fails.

## Scope

Own `portal-types/src/agents.rs`, `src/herdr_socket.rs`, `src/transcript.rs`, `src/activity.rs`, `src/web/sessions.rs`, `src/web/mod.rs`, `src/web/agents.rs`, `web/src/conversations.rs`, `web/src/agent_view.rs`, and `web/src/api.rs`.
Update only required `AgentView` construction fixtures in `src/web/inbox.rs` and `web/src/board.rs`. No generated files or dependencies. All other code and configuration are outside scope.
The frontend changes are session application and data binding only. Do not implement the selected visual layout yet. Preserve independent transcript loading, pending controls, attachments, and follow-at-bottom behavior.

## Acceptance

Run `cargo test --bin dogsled herdr_socket::`, `cargo test --bin dogsled transcript::`, `cargo test --bin dogsled activity::`, `cargo test --bin dogsled web::`, `cargo test -p portal-types`, and `cargo test -p web`.
Add focused tests for malformed/failed native lists, confirmed absence, actual bounded-read failures, valid empty history, custom and unsaved Pi sessions, native replacement during a read, metadata/transcript arrival order, late responses, restart epochs, and unchanged turns with changed truncation/session metadata. Exercise a failing reader seam rather than an open-only preflight.
Run formatting and scoped strict Clippy. The lead owns bundle generation, browser proof, and the final full gate. Do not run builds or installations except the repository's enabled commit hooks.

## Delivery

Use the seeded unit and assigned branch. Preserve unrelated commits and work. Commit with hooks enabled and push only that branch. Do not open a PR, merge, release, deploy, or alter hooks/test budgets. Report exact SHA, changed paths, each command's result at the tested revision, the session-registry API available to the next unit, and remaining limits.
