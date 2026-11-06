# U5: reviews and questions follow the inbox

Base: `0394bc9` (`integrate/files-refresh`: `main` `73dea6c`, then U1 through U4a). Line numbers are at that base.

## Outcome

No timer fetches reviews or questions. The open review is refetched only when its inbox revision changes. The question form makes no request of its own: it shows the `Pending` item that the agent view already holds. The agent view refetches its review and question lists only when this agent's set of (inbox id, revision) changes.

## Cause

- `ReviewView` (`web/src/review_view.rs:111-130`) reads `/api/reviews` every 2 s (`POLL`, `:43`) and bumps `tick` on each read (`:123`). `ReviewLoader` then refetches the open review on every tick (`:193`).
- `QuestionForm` (`web/src/question_form.rs:66-79`) reads `/api/questions` every 2 s (`POLL`, `:50`).
- The portal stream already carries every pending review and question as an inbox item (`src/web/inbox.rs:59-125`). The item has no revision, so a new review revision can't be seen from the stream.
- Both components are used only embedded in `AgentSession`, always with an id (`web/src/agent_view.rs:564`, `:666`). Their list modes (`review_view.rs:149-166`, `question_form.rs:97-125`) are not reachable.

## Edits

1. **Inbox revision on the server:**
   - In `portal-types/src/inbox.rs`, add `#[serde(default)] pub revision: u64` to `InboxItem`. Its doc comment says: "the review's number of submitted revisions; 0 for other kinds".
   - In `src/web/inbox.rs` `view`, set it from `review.revisions.len()` for a review, and to 0 for every other kind.
2. **Inbox revision in the web app:** in `web/src/app.rs`, add `revision: u64` to the web `InboxItem` and fill it in `InboxState::items`. Update the test constructors.
3. **Agent view:**
   - In `web/src/agent_view.rs`, make `agent_inbox_ids` key on (id, revision).
   - Pass `ReviewView` the `revision` of the matching inbox item.
   - Pass `QuestionForm` the `Pending` with the requested id, taken from `questions()`. While it is absent, show the existing loading `Status`.
4. **`web/src/review_view.rs`:**
   - Delete `POLL`, the loop, `tick`, the list mode, `selected`, `opened` and `loading`.
   - `ReviewView` takes `review_id: String` and `revision: u64`. `ReviewLoader` fetches on `(id, revision)` only.
   - `ReviewEditor` already keeps the PUT reply (`:719-727`). Delete the `on_done` chain that only removed the list entry.
5. **`web/src/question_form.rs`:**
   - Delete `POLL`, the loop, `fetch_pending`, the list mode, `opened`, `current_form`, and anything else only the list mode uses.
   - `QuestionForm` takes `question: Pending` and `on_done`.
   - If the form shows a ticking elapsed time, update only a clock signal with `use_interval`. That clock makes no request.

## Paths

**Owns:** `portal-types/src/inbox.rs`, `src/web/inbox.rs`, `web/src/app.rs` (`InboxItem` and `items` only), `web/src/agent_view.rs` (the pending wiring, `agent_inbox_ids`, their tests), `web/src/review_view.rs`, `web/src/question_form.rs`, `web/src/component_tests.rs`.

**Reads:** `portal-types/src/reviews.rs`, `portal-types/src/questions.rs`.

## Acceptance

- In `src/web/inbox.rs`, add a test: a review with two revisions gives `revision: 2`, and a question gives 0.
- In `agent_view.rs`, add a test: a revision change alters the inbox key, and an age or title change does not.
- Update the component tests for the new props.
- `rg -n 'POLL|sleep\(|tick|fetch_pending|loop \{' web/src/review_view.rs web/src/question_form.rs` finds nothing.
- `cargo test -p web`, `cargo test -p portal-types` and `cargo test --bin dogsled web::inbox` pass.

## Checks

Run `cargo fmt --all --check` and `cargo clippy --all-targets -- -D warnings`. The lead prebuilds `web/public` and runs `moon run dogsled:web-lint`; don't run `web-build`. Commit through the pre-commit hook; don't bypass it. This brief is complete; it depends on no plan file.
