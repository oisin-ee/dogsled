# Brief: plan reviews store and API (web portal step 5a)

**Outcome.** `dogsled web` keeps plan reviews with revisions, line diffs between revisions, draft annotations with image attachments, and a decision, and streams each decision to the submitting Pi session. The Pi plan mode is step 5b; the UI is step 5c.

**Design.** Plan `plans/web-portal.md`, Approach 5. Store, token rule and Origin rule come from step 4a (`src/web/store.rs`, `src/web/questions.rs`) — reuse them; if 4a has not landed on your base, stop and report. Diff with `similar = "3.2.1"` (line diff). Attachments under `<state>/web/attachments/<uuid>.png`, mode 0600, max 10 MiB, PNG/JPEG/WebP checked by magic bytes.

**Edits.**
- `Cargo.toml`: add `similar = "3.2.1"`.
- New `portal-types/src/reviews.rs` (re-export from lib): `Review { id, pane, session, cwd, path, revisions: Vec<Revision>, annotations: Vec<Annotation>, general: String, decision: Option<Decision> }`, `Revision { n, content, submitted_ms }`, `Annotation { id, revision, start_line, end_line, quote, comment, images: Vec<String> }`, `Decision::Approved { notes } | ChangesRequested { message }`, `LineChange { kind: Same|Added|Removed, old: Option<usize>, new: Option<usize>, text }`.
- New `src/web/reviews.rs`:
  - `POST /api/reviews` (token; `{path, content, pane, session, cwd}`): same `session` + `path` with no decision appends a revision to the open review and carries over annotations whose quoted lines are unchanged (re-anchor line numbers via the diff; drop ones whose lines changed); otherwise opens a new review. Returns `{id, revision}`.
  - `GET /api/reviews`, `GET /api/reviews/:id`, `GET /api/reviews/:id/diff?from=n&to=m`.
  - `PUT /api/reviews/:id/annotations` (Origin; full draft list), `PUT /api/reviews/:id/general` (Origin).
  - `POST /api/attachments` (Origin; raw image body → `{path}`), `GET /api/attachments/:name`.
  - `POST /api/reviews/:id/approve` (Origin; `{notes?}`) and `POST /api/reviews/:id/request-changes` (Origin). Request-changes builds one message: each annotation as `> quote` with `plan lines a–b`, the comment, then a "Reference images" list of absolute attachment paths, then the general comment.
  - `GET /api/sessions/:session/decisions` (token; SSE event `decision` `{review_id, path, decision, message, plan_content}`; replays undelivered decisions on connect; `POST /api/decisions/:review_id/ack` (token) marks delivered).
- `src/web/mod.rs`: route registration only.

**Owned paths.** `Cargo.toml`, `Cargo.lock`, `portal-types/src/reviews.rs`, `portal-types/src/lib.rs` (one line), `src/web/reviews.rs`, `src/web/store.rs` (generalise only if needed), `src/web/mod.rs` (routes).

**Acceptance.** `cargo test --workspace` passes, including: resubmitting the same path adds revision 2 and carries an annotation on an unchanged line with its new line number while dropping one on a changed line; request-changes message format (quote, line range, comment, image paths); attachment rejects a non-image and writes 0600; a decision is replayed on SSE reconnect until acked; Origin/token 403s.

**Checks.** `moon run :fmt-check :lint :test`.

**Base.** origin/main after step 4a lands (the lead fills the SHA at dispatch).
