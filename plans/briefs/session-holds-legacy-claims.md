# Session-scoped holds and no requestless preparation claims

Base: `origin/main` at `385d6dd3b2f8e5fbaf4f611351635cefbd85481b`. Branch: `fix/session-holds-legacy-claims`.

## Outcome

A session's board, sidebar, and prompt banner show only the resource holds that the session owns. Dogsled no longer has a requestless preparation claim type.

## Cause

1. `src/board.rs:905` (`resource_rows`) keeps a hold when `hold.owner_pane.as_deref().is_none_or(mine)`. Every hold that has no owner pane therefore appears in the board of every owner. `board::build` has the doc comment "The board for `owner`". The user sees 73 "blocked" cards in each new session and none of them are agents.
2. `src/prepare.rs:30` `Pending.request_id` is `Option<String>`. New requestless preparations are already refused (`prepare.rs:1110`, `prepare.rs:1589`). The only `None` records are legacy ones, and no command can release them (`holds.rs:56` rule `legacy_preparation`, `holds.rs:655`). The user decided to drop legacy support. Do not add a migration or compatibility path.

## Edits

- `src/board.rs` `resource_rows`: when `owner` is `Some(o)`, keep a hold only when `hold.owner_pane.as_deref() == Some(o)`. When `owner` is `None` (`--all`), keep every hold. Attaching holds to rows by `request_id` does not change. Add a board unit test: an unowned hold is absent from `build(.., Some("w1:p1"), ..)` and present in `build(.., None, ..)`. Also include a hold owned by another pane and a hold owned by `w1:p1`.
- `src/prepare.rs`: make `Pending.request_id` a required `String`. Narrow `record` and the `Option<&str>` plumbing that exists only to feed `Pending`. Keep any `Option` that has a different meaning, for example `saved_tree`'s `request` filter. Find the sites with `git grep -n 'request_id' src/prepare.rs src/prepare/`.
- `src/holds.rs`: remove the `legacy_preparation` rule and the `(None, _)` branch in `preparations`. A claim without `request_id` is an unreadable record: report it with the existing `unreadable` code, not a special case.
- Tests: `tests/cli/preparation_retirement.rs:565` (`requestless_and_unknown_claims_refuse`) and `tests/cli/stop_sweep.rs:95` (`legacy_preparation`). Remove the requestless parts and keep the unknown-claim assertions. Run `git grep -n 'requestless\|legacy_preparation' src tests docs README.md` and update every remaining hit that is about stored claims. The refusal messages for new requestless *requests* stay.
- `CHANGELOG.md`: add an Unreleased entry, one sentence for each change.
- `docs/lifecycle.md`: if it describes requestless or legacy preparation holds, delete that text.

## Owned paths

`src/board.rs`, `src/prepare.rs`, `src/prepare/`, `src/holds.rs`, `tests/cli/preparation_retirement.rs`, `tests/cli/stop_sweep.rs`, plus any test file that the grep above finds. Also `CHANGELOG.md` and `docs/lifecycle.md`.

## Acceptance

`cargo test --bin dogsled board:: && cargo test --bin dogsled prepare:: && cargo test --bin dogsled holds:: && cargo test --test cli preparation_retirement:: stop_sweep::` all pass. Before the fix, the new board test fails.

## Checks

Run `cargo fmt --check` and `cargo clippy --all-targets -- -D warnings`, then the acceptance tests. Commit with a conventional message and push the branch. Do not open a PR, release, or touch host state under `~/.local/state`.
