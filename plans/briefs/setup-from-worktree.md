# Brief: run worktree setup from the new worktree's revision (#221)

**Goal.** When `place`, `spawn`, or `run` creates a worktree, the setup commands come from the `dogsled.toml` of the new worktree's revision, not from the host's main checkout. Issue: https://github.com/oisin-ee/dogsled/issues/221.

**Cause.** `src/prepare.rs` loads `crate::repository::load(&checkout)` (line 1229) and `crate::repository::load(&repository)` (line 1027). Both read `<main checkout>/dogsled.toml`, which is out of date when the main checkout lags `origin`. Line 1459 then runs `settings.worktree.setup` from that file.

**Base.** `origin/main` at the commit that adds this brief. Branch `fix/setup-from-worktree`.

**Edits.**

1. `src/prepare.rs`: `worktree.dir` still comes from the main checkout, because the path is needed before the worktree exists. After a worktree is created, and before setup runs (near line 1459), load the settings again from the new worktree's path with `crate::repository::load`. Run `setup` from those settings.
   - A missing `dogsled.toml` in the new worktree means no setup.
   - An invalid one is a setup failure, with the same cleanup and `Cause::SetupFailed` diagnostic as a failed command.
2. Teardown (`src/worktree.rs:931`, `src/close.rs:672`) does not change. Name it in the PR body as follow-up if you think it has the same defect.
3. Tests in `tests/cli/place.rs`, next to `place_creates_a_locked_worktree_runs_setup_opens_it_and_records_the_anchor`:
   - **Base has newer setup.** The main checkout's `dogsled.toml` has `setup = ["false"]`. The base commit's `dogsled.toml` has a setup that writes a marker file. `place --branch … --base <that commit>` succeeds, and the marker exists in the new worktree.
   - **Base has no file.** The main checkout has `setup = ["false"]`, and the base commit has no `dogsled.toml`. `place` succeeds and runs no setup.

**Owned paths.** `src/prepare.rs`, `tests/cli/place.rs`.

**Not owned.** Everything else, including `pi/`, `docs/`, and `plans/`. If a change elsewhere seems necessary, stop and report. Another writer owns `pi/` and `docs/` at the same time.

**Your checks.** `cargo fmt --all --check`, `cargo clippy -p dogsled --all-targets`, `cargo test -p dogsled --test cli place`, and `cargo test -p dogsled --test cli launch_retry`. Do not run the full suite, a release, or an install. The lead runs the full acceptance.

**Publication.** Commit with `fix(prepare): run setup from the new worktree's dogsled.toml` through the normal hook. Push the branch and open a draft PR that says `Fixes #221`. Do not merge, tag, or publish.
