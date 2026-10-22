# Brief: handoff stops failing on a missing local branch

Base: `origin/main` 0dc9206. Branch: `fix/handoff-missing-branch`. Lead: wJ9:p2S.

## Problem (measured)

The 0.23.0 service on netcup logged 3352 lines in 11 minutes, across 35 distinct
branches. 0.22.0 logged at the same rate, so this is not new:

    dogsled handoff: branch <b>: <repo>: cannot resolve integration tips:
    fatal: ambiguous argument 'refs/heads/<b>': unknown revision or path not in the working tree.

Examples: in `~/dev/tova`, `docs/live-audit-e5` and `docs/live-audit-e5-android` exist only
on origin, with no local ref and no worktree. In dogsled, `docs/calibration` has no local ref.

## Cause (read in src/done.rs at 0dc9206)

- `handoff_locked` (~:524) calls `refresh_branch` for **every** mark with a branch on
  **every** tick, before its `closed_ms` check. Closed marks stay in `done.json` and are
  refreshed forever.
- `refresh_branch` (~:316) always asks git for `refs/heads/<name>`. When the local ref is
  gone, because cleanup deleted it, it was only pushed, or the worktree went away, `rev-parse`
  fails in `integration::check_cached`. The error is logged and the mark is saved unchanged,
  so on the next tick the same thing happens again.
- A closed mark whose branch is `!landed` still shows on the board as "branch +N to land"
  (`board.rs` ~:705). So skipping the refresh alone would leave stale rows. The fix must
  decide what a missing local ref *means*.

## Required behaviour

1. A missing `refs/heads/<b>` is a known state, not an error. Find out from git (show-ref):
   - if the remote-tracking ref `refs/remotes/origin/<b>` exists, assess that tip, so landed
     and ahead stay true for a pushed-only branch;
   - if neither ref exists, the branch has nothing left to land. Record that durably on the
     mark (pick the representation in `Branch`; serde must still read old `done.json`), and
     the board must not show it as "to land".
2. Once a mark is closed and its branch is settled (landed, or gone), `handoff_locked`
   no longer refreshes it on each tick.
3. Real git failures (corrupt repo, missing repo directory) are still logged. Do not
   silence the log line or lower its level as the fix.
4. Open and unclosed marks behave as before.

## Acceptance (tests in `src/done/tests.rs`, fake or real git as the existing tests do)

- A closed mark whose local branch was deleted after it landed: one tick sets it
  settled, and a second tick runs no `rev-parse` for it and logs nothing.
- A pushed-only branch (origin ref only, one commit ahead of base): ahead = 1, not landed,
  no error.
- A branch with neither ref: settled, no error, and not counted as "to land".
- Each new test fails on 0dc9206. Say how you checked.
- Full gate at your final head: `cargo fmt --check`,
  `cargo clippy --locked --all-targets -- -D warnings`, `cargo test --locked`. Report counts.

## Scope

Owns: `src/done.rs`, `src/done/tests.rs`; `src/integration.rs` / `src/loss.rs` only if the
ref resolution belongs there. Do **not** edit `src/work.rs`, `src/board.rs`,
`src/commands.rs` or `CHANGELOG.md`, because another writer has them. If the board needs a
change, stop and tell the lead.

## Delivery

Commit through the hooks (never `--no-verify`), push, and open a draft PR against main. Reply
with the head SHA, the PR number and the gate counts, then call `done`.
