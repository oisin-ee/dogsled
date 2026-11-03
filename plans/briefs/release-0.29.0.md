# Release dogsled 0.29.0: version bump commit

**Outcome.** One commit `chore(release): 0.29.0` on branch `chore/release-0.29.0`, based on `origin/main`. It sets every version string to 0.29.0 and adds a dated CHANGELOG section. The lead pushes, tags and opens the PR, not you.

**Cause.** `origin/main` has the per-host SQLite store (PR #240, `dec6504`) and retention expiry (PR #238, `f69bb10`), but the latest tag is `v0.28.1` (`0098f69`), which has neither. The 0.28.1 service rewrites an 8 MB `queue.json` on every pass and holds the queue lock, so tickets stall. Releases come from annotated `vX.Y.Z` tags (`.github/workflows/release.yml`). The version must be on `main` when the tag is pushed.

**Edits.** Use the same files as the 0.28.1 release (`git show 6aeeac8 --stat`):
1. `Cargo.toml`, `package.json`, `herdr-plugin.toml`, `packages/oisinpi/package.json`, `packages/pi-guardrails/package.json`, `packages/pi-prewalk/package.json`, `packages/pi-tiers/package.json`, `plugins/dogsled/.claude-plugin/plugin.json`, `plugins/dogsled/.codex-plugin/plugin.json`: set the version to `0.29.0`. Check whether `packages/pi-dogsled` or `packages/pi-todo` carry a version; if the 0.28.1 commit did not touch them, do not touch them.
2. `Cargo.lock`: run `cargo update -p dogsled --precise 0.29.0 --offline`, or `cargo check --locked` after the edit fails and you retry without `--locked` once. No other lock changes.
3. `README.md` and `scripts/npm-retire.sh`: change `0.28.1` / `v0.28.1` to the new version where the 0.28.1 commit did.
4. `CHANGELOG.md`: add `## 0.29.0 - 2026-10-29` above `## 0.28.1`. Write the entries from `git log v0.28.1..origin/main` and `plans/sqlite-state-store.md`. Cover: host items and the queue live in one SQLite database in the state directory; **the store starts empty and does not import `queue.json` or the old item files, so open items and tickets from 0.28.1 are not carried over**; retention expiry of released terminal lifecycle records; the fix for preserving recorded ordinary handoffs after closure. State only what the commits do. Write in short, plain sentences.

Find any missed site with: `rg -n '0\.28\.1' -g '!CHANGELOG.md' -g '!plans/**' -g '!target/**' -g '!node_modules/**'`. It must print nothing when you are done, except text that names the older release on purpose. Report any such hit.

**Owned paths.** The files above, and nothing else.

**Acceptance.** `cargo check --locked` passes. The `rg` command above is clean. `git diff origin/main --stat` lists only owned paths.

**Checks.** The pre-commit hook runs the repository checks. Let it run once. Run the commit as a Dogsled `run` job with a 20 minute timeout if the shell times out. Do not bypass the hook. Do not run the checks a second time by hand.

**Publication.** Commit only. No push, no tag, no PR. Do not install, upgrade, or change any local version pin or the running service.

**Base.** `origin/main` at `385d6dd`. Create the branch from it in a worktree under `.worktrees/`. Reply with the branch, the SHA, the changed paths and the command results, then call done.
