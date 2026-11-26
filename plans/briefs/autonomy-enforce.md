# Refuse and release owner requests while an agent is autonomous

Plan: `plans/autonomous-toggle.md` (approved), sections "What an autonomous agent does" and "When a switch changes".
Needs the publication unit merged (`plans/briefs/autonomy-publish.md`): `autonomy::cause_of(ledger, pane)` and `autonomy::record`.

## Outcome

An autonomous agent's `interview` shows no form, its submitted plan is approved at once with a recorded decision, requests that already wait are released when a switch goes on, and each affected agent gets one inbox message.

## Edits

1. `src/web/questions.rs:38` (`create`): when `cause_of(body.pane)` is `Some`, answer 409 with `{"code":"autonomous"}` and store nothing.
2. `src/web/store.rs`: `release(id)` marks a pending question `released` (the state exists since schema 9) and wakes its waiter (:257). `Poll` gains `Released`. `answer_wait` (`src/web/questions.rs:66`) answers 200 with `{"id":…,"released":true}`. A released question is not pending, so it leaves the inbox.
3. `src/web/reviews.rs:439` (`submit_plan`): store the review as today. When the pane is autonomous, approve it in the same call with the note "Approved by autonomous mode", through the code path of `approve` (:279), so that the decision stream wakes the session.
   Then call `autonomy::record` with source `plan`, the review id, the title `Approved its own plan: <path>`, the decision "Approved without the owner.", and the why "Autonomous mode is on."
4. `src/autonomy.rs`: `apply_changes(ledger, shell, before, after, now)`. The service calls it after each publication with the previous and the new `panes` map of the `autonomy` view. For each local pane:
   - Off to on: release its pending questions, approve its pending reviews as in edit 3, and send one inbox message when the agent is working or had such a request.
     Text: the Autonomous block of `src/context.rs`, with the first sentence "Autonomous mode is now on."
   - On to off: send one message when the agent is working: "Autonomous mode is now off. Ask your owner again when a question is open."
   - Send with `inbox::deliver` (`src/inbox.rs:314`) and a host-made `Record`, as `src/transfer/peer.rs:244` does. The sender name is `dogsled`. The first publication after a service start has no previous map: send nothing, but still release and approve.
5. `pi/interview.ts`: a 409 with code `autonomous` from `POST /api/questions`, and a poll answer with `released: true`, both return a normal tool result with status `autonomous` and this text. They are not errors.
   "Autonomous mode is on. No form was shown. Do not ask again. Choose the most reasonable answer from the plan, the request, and the code. Record it with the decide tool, then continue."
   `outcome` (:451) and `wait` (:483) are the two places. Do not upload images before the refusal is known: ask first, upload only when the question is accepted.

## Paths

Own: `src/`, `tests/`, `pi/`. Read: `portal-types/`, `plans/`. Do not change `portal-types/` or `web/`.
`pi/plan-mode.ts` needs no change: an approval already reaches the session through the decision stream (:226).

## Acceptance

- `cargo test autonomy` passes, with tests on a real temporary ledger, in the fixture style of `src/web/questions.rs:294` and `src/web/reviews.rs:1151`:
  - `create` for an autonomous pane answers 409 `autonomous` and the store stays empty.
  - A waiting long-poll answers `released` after `release`, and the question leaves `pending`.
  - `submit_plan` for an autonomous pane leaves the review approved and one `plan` decision with its review id.
  - `apply_changes` from off to on releases a pending question and queues one inbox record; a second call with the same maps queues none.
- `node --test pi/interview.test.ts` passes with two new cases: the refused form and the released form both return status `autonomous` and withdraw nothing.

## Checks

`cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test autonomy`, `cargo test questions`, `cargo test reviews`, `pnpm run --silent typecheck`, and `node --test pi/interview.test.ts`. Run no other test and no build.
