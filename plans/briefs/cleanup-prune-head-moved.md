# Brief: the prune path still reports a new commit as "planned worktree identity changed"

Repository: dogsled
Base: origin/main `6708c1e` (it includes PR #121). Run `git fetch origin` first, and check that `git merge-base --is-ancestor 6708c1e HEAD` succeeds.
Branch: `fix/cleanup-prune-head-moved`

## Evidence
- PR #121 (`b21bb37`) fixed `validate_tree` (`src/cleanup.rs` about line 1658). There, a HEAD that moved while repo, path and branch stayed the same is now `Recheck::Refused`, which is a skip with `refuse <path>: HEAD moved since the survey (<old> -> <new>)`. A real identity change is still a failure.
- `apply_one` (`src/cleanup.rs` about line 2028 on 6708c1e) still has `if current.tree != requested.tree { return Err(... "planned worktree identity changed") }`.
  - `Entry.tree` is an `Option<Tree>`, and `Tree` includes `head`.
  - So in the prune path, an agent that commits between the plan and the apply still produces a failure and an owner alert. That is the same misclassification that #121 fixed, and the rule from `3232960` and #121 applies here too.
- The #121 child also found that `delete_branch` has "branch identity changed" (about line 1839). Check whether it compares a whole struct that includes a head or tip. Change it only if the evidence shows the same cause: live work moved a tip on an unchanged identity.

## Deliverable
- **`apply_one`:**
  - A changed repo, path, branch or presence of a tree stays a failure (`planned worktree identity changed`).
  - A changed `head` only becomes `Outcome::Skipped` with the same `HEAD moved since the survey (<old> -> <new>)` wording as #121.
  - Share one helper with `validate_tree` so the two checks cannot drift apart. For example, a `Tree` method that returns identity-changed, head-moved or same.
- **Tests in `src/loss_tests.rs`, through the prune path** (the entry point that reaches `apply_one`):
  - A commit after the plan is skipped and the worktree stays.
  - A branch switch after the plan is still a failure and the worktree stays.
- **Docs:** add a `CHANGELOG.md` Unreleased line. Update `README.md` and `docs/design.md` only where they describe this outcome.
- **Owned paths:** `src/cleanup.rs`, `src/loss_tests.rs`, `CHANGELOG.md`, `README.md`, `docs/design.md`.

## Rules
- Never remove, prune or force anything in a real worktree.
- Keep the recheck as strict as it is now: a moved worktree is never removed in that pass.
- Do not bypass hooks. If a hook fails for a reason outside these paths, stop and report it.

## Acceptance
- `cargo test --locked`, `cargo clippy --all-targets -- -D warnings` and `cargo fmt --check` all pass.
- Commit with `fix(cleanup): treat a commit after the plan as a skip on the prune path too`. Push it and open a draft PR against `main` that includes this evidence.
- Reply with the PR URL and the head SHA, then call `done`.
