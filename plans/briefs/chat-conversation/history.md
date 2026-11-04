# Preserve conversation timeline records

## Outcome

The conversation endpoint preserves message identity, time, source, and compact tool history for the selected timeline.

## Cause and evidence

`portal-types/src/agents.rs:55–68` has only role/text. `src/activity.rs:211–267,402–444` removes metadata and custom deliveries. `src/web/agents.rs:145–164` reconstructs role/text.
`pi/inbox.ts:11–20,145–150` already records Dogsled sender, kind, ID, text, and creation time. These records are currently absent from the portal.
Read `plans/chat-conversation.md`, the current parser tests, the endpoint tests, and `web/src/conversations.rs` before editing.
The approved layout is `.impeccable/mocks/decision/chat-conversation/timeline.html`. This unit supplies its records, not its rendering.

## Edits

- Extend the shared conversation records with stable ID, optional timestamp plus its meaning, and typed source metadata. Keep role/text for message content. Distinguish context and tool records from owner and assistant messages. Use one typed representation, not a second duplicate timeline beside turns.
- Use native entry/message IDs where available. An absolute transcript record offset is the fallback, not its position in the returned array. Preserve source order and existing Claude block grouping.
- Preserve Pi nested message timestamps and available assistant model/provider metadata. Entry timestamps describe recording time. Parse ISO timestamps with existing `chrono`, not a handwritten parser. Missing or invalid times stay absent.
- Parse displayed Pi `custom_message` entries. For `customType: "dogsled"`, split validated `details.records`, preserve each sender/kind/created_ms, and deduplicate repeated record IDs. Never parse sender identity from prose or expose hidden `display:false` content.
- Preserve valid Claude envelope timestamps and message model. Preserve Codex response-item envelope timestamps as recorded time. Do not substitute the current model or clock where provenance is absent.
- Include compact typed tool summaries from supported recorded tool calls/results. Correlate by tool-call ID; preserve name and known result/error status. Do not return raw arguments, output, or reasoning text. An unmatched call means recorded call with unknown completion, never currently running. Keep text and tool rows distinct.
- Retain bounded reads: 32 MiB and 200 text messages. Bound tool summaries independently, and expose truncation when the returned history is incomplete. Tool-heavy tails must not remove the last useful text messages. Do not change active-branch semantics in this unit.
- Return the enriched records directly from the endpoint. Update every owned `Turn` constructor and cache fixture for compilation. Preserve independent transcript loading and existing cache behavior.

## Owned paths

`portal-types/src/agents.rs`, `src/activity.rs`, `src/web/agents.rs`, `web/src/conversations.rs` (test fixture changes only), and `tests/cli/transcripts.rs` if a regression belongs there. No generated files.
Do not edit UI rendering, CSS, lifecycle, dependency versions, installed tools, or other agents' work. Ask the lead before expanding scope.

## Acceptance

From the repository root: `cargo test --bin dogsled activity::`, `cargo test --bin dogsled web::agents::`, `cargo test -p portal-types`, and `cargo test -p web conversations::` all pass.
Add focused fixtures for Pi/Claude/Codex times and IDs, absent/invalid metadata, model changes, Claude grouping, visible/hidden custom messages, attributed multi-record Dogsled deliveries, repeated delivery IDs, tool correlation/error/unknown completion, truncation, and oversized tool tails. Assert the endpoint preserves metadata rather than only testing helpers.

## Checks and authority

Allowed: acceptance commands, `cargo fmt --all --check`, focused Clippy, and the repository's enabled commit hooks. A bundle build is allowed only when required to supply the root crate's embedded assets; use worktree-local Cargo output.
Do not run the final full repository gate, browsers, servers, installations, or hosted CI. The lead owns integrated browser evidence and the final gate.
Commit the complete unit with hooks enabled. Push only the assigned feature branch. Do not open a PR, merge, release, or deploy.
Report the branch, exact head SHA, changed paths, command results at that revision, the resulting wire contract, and remaining limits. Stop at an owning blocker; do not ship a workaround.

## Base

The dispatch contract supplies the exact committed base. Prerequisites: the selected design and reviewed metadata investigation. The user approved the timeline, not a rewrite of lifecycle or transcript storage.

## Primary reference

The lockfile pins Pi 1.3.0. Verify relevant fields against its [session format](https://github.com/earendil-works/pi/blob/71023aec1546d6d677a65669c5665c0927ce0dd0/packages/coding-agent/docs/session-format.md) and linked message types. Keep a concise research note in the final report for the lead's PR body.
