# Brief: cleanup reports a new commit as a failure ("worktree identity changed")

Repository: dogsled (`/Users/oisin/dev/dogsled` locally, or your host's clone)
Base: origin/main `6248dbeeab8efa8f0d04801917ce4a23111eb72f`. Run `git fetch origin` first and check that the base contains it.
Branch: `fix/cleanup-head-moved`

## Evidence (live, 2026-10-22)
- The cleanup pass started at 1792606107 s and applied at 1792606208 s. It failed with:
  `refuse /Users/oisin/dev/shoebox/.worktrees/int-app: worktree identity changed`.
- The worktree reflog shows that its owner committed a merge (`5656a90`, branch `int/rust-app`) at 1792606172 s. That is between the survey and the apply.
  - At survey time, HEAD was `a8fd6e9`. That commit is on `main`, so the worktree was landed and was planned as removable.
  - Repo, path and branch did not change. Only `head` moved.
- `src/cleanup.rs` `validate_tree` (about line 1658) compares the whole `Tree` with `current != *planned`. `Tree` has the fields `repo`, `path`, `branch` and `head`.
  - So a commit from live work is classified as an identity change and goes into `Applied.failures`. The owner then gets an alert.
- Commit `3232960`, "fix(cleanup): report a recheck refusal after the plan as a skip, not a failure", already decided this rule:
  - State that changed through live work (now dirty, held, not landed, or busy) is a skip.
  - Identity changes stay failures.
  - A HEAD that moves forward because of a new commit is live work of the same kind as "not landed". It is not an identity change.
  - The test `worktree_identity_change_after_the_plan_still_fails` (`src/loss_tests.rs` about line 871) creates that case with an empty commit. It encodes the misclassification.

## Deliverable
- **Split the recheck in `validate_tree`:**
  - A changed `repo`, `path` or `branch`, a different main checkout, or an empty head stays a failure (`worktree identity changed`).
  - A changed `head` with the same identity becomes `Recheck::Refused` with a specific reason, such as `refuse <path>: HEAD moved since the survey (<old> -> <new>)`. That makes it a skip, and the next survey re-plans it.
- **Check `src/cleanup.rs` about line 1839 ("branch identity changed") and line 2020 ("planned worktree identity changed")** for the same whole-struct comparison. Apply the same split only where the evidence shows the same cause.
- **Tests in `src/loss_tests.rs`:**
  - A commit after the plan is a skip with the HEAD-moved reason, and the worktree stays.
  - A real identity change after the plan is still a failure, and the worktree stays. Use a branch switch, or a path re-registered to another branch.
  - Replace or rename the test that encoded the old behaviour.
- **Docs:** update `CHANGELOG.md` (Unreleased). Update `README.md` and `docs/design.md` wherever they list which recheck outcomes are skips and which are failures.
- **Owned paths:** `src/cleanup.rs`, `src/loss_tests.rs`, `CHANGELOG.md`, `README.md`, `docs/design.md`.

## Rules
- Never remove, prune or force anything in a real worktree. Do not change the recheck itself: a moved worktree must still never be removed in that pass.
- Do not bypass hooks. If a hook fails for a reason outside these paths, stop and report it.

## Acceptance
- `cargo test --locked` passes, including the new `loss_tests` cases.
- `cargo clippy --all-targets -- -D warnings` and `cargo fmt --check` are clean.
- Commit with the message `fix(cleanup): treat a commit after the survey as a skip, not an identity change`. Push it and open a draft PR against `main` that includes this evidence. Reply with the PR URL and the head SHA, then call `done`.
