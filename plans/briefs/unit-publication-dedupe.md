# U1b: publish a new revision only when content changes

Plan: `plans/git-files-sidebar.md`, unit U1b. Base: `43a2254` (`origin/feat/remote-detail-follow`, U2).

## Outcome

`Publisher::replace` assigns a new `revision` and notifies subscribers only when the publication content or the activity content changes. Two known test races are fixed where they live.

## Cause

- `src/observation.rs:149` always increments `revision`; `:201` always calls `send_replace`. A DETAILS-only cycle (`src/service.rs:333-360`, `diagnostics.rs:23`) therefore republishes identical content under a new revision, so every portal subscriber reloads. A trace showed revisions 2 and 3 with identical JSON apart from the revision fields.
- That same behaviour makes `interests_scope_ticks_and_viewers_share_one_capture` fail at its revision comparison (`src/observation/tests/details.rs:143`, revision 3, expected 2).
- `shutdown_closes_an_idle_agent_subscription` (`src/web/agents.rs`): `Fixture::wait` (`src/observation/details_fixture.rs:105-111`) accepts the retained empty frame from before registration. A trace showed this ordering; it fails about 1 run in 100.

## Edits

1. `src/observation.rs` `Publisher::replace`: before assigning a revision, compare the new content with the previous frame's content.
   - Content includes `local`, `gaps`, `views`, and all of `sources` (measurement times, receipts, availability and retry acknowledgements are freshness evidence).
   - Content excludes the main `published_ms` (`collect.rs:162` sets it each cycle) and the `epoch`, `revision` and `published_ms` that `replace` writes into the `portal` view snapshot.
   - Activity content is `self.activity` (`:120-121`). Exclude `epoch` and `revision`, which `replace` writes. Do not compare a field that decreases with time (`valid_for_ms`, `live_activity.rs:529-535`); compare the stable deadline it is derived from. A renewal must still publish.
   - If both are equal, keep the existing frame unchanged: same `Arc`, same `activity_measured`, no `send_replace`. Return the previous `Published`.
   - Keep `local_revision` behaviour.
2. `a_slow_detail_capture_does_not_delay_portal_publication` (`src/observation/tests/details.rs:386`) calls `Fixture::republish()` with unchanged input and waits for a higher revision. Change it to make a real metadata change while the capture is blocked (update the helper in `src/observation/details_fixture.rs`). Keep its assertion that publication progresses while a capture is blocked.
3. `src/web/agents.rs:1612` `shutdown_closes_an_idle_agent_subscription`: before `state.stop.cancel()`, wait for a publication whose `agent_details.data` contains `w1:p1`, and record its revision. After the cancel, wait for a newer publication with empty `agent_details`. Keep `assert!(fixture.no_viewers())`. Do not add a sleep.
4. Do not weaken any read-count assertion.

## Paths

Owns: `src/observation.rs`, `src/observation/tests.rs`, `src/observation/tests/details.rs`, `src/observation/details_fixture.rs`, `src/observation/tests/live_activity.rs`, `src/web/agents.rs` (that one test only).
Reads: `src/service.rs`, `src/observation/collect.rs`, `src/observation/project.rs`, `src/observation/live_activity.rs`, `src/observation/ipc.rs`, `src/observation/diagnostics.rs`.

## Acceptance

Add tests in `src/observation/tests.rs`:
- `replace` with identical input twice keeps the revision and wakes no subscriber (`watch::Receiver::has_changed` is false).
- Only `published_ms` changed: no new revision.
- A changed source measurement time, gap, view or local value: new revision.
- Activity: an unchanged view gives no new revision; a renewal gives a new revision.
These existing tests must still pass without weakening: `tests.rs:84-95`, `:160-227`, `:274-296`, `:355-365`; the changed-screen test in `tests/details.rs`; `diagnostics.rs:108-136`; `tests/peer.rs:124-139`; `src/web/oneshot.rs:305-312`; `tests/live_activity.rs:473-493`.

Run `cargo test --bin dogsled observation::` and `cargo test --bin dogsled web::`. Then run the two raced tests 200 times each with `--exact`; there must be 0 failures.

## Checks

`moon run dogsled:fmt-check`, `cargo clippy --all-targets -- -D warnings`, the tests above. `web/public` is prebuilt by the lead; do not run `web-build`. Commit through the pre-commit hook; do not bypass it.
