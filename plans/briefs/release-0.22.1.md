# Release dogsled 0.22.1: version bump PR

**Outcome.** A merged PR on `main` with the commit `chore(release): 0.22.1`. It sets every version string to 0.22.1 and dates the CHANGELOG section. The lead pushes the tag after the PR merges, not you.

**Cause.** PR #136 (`4e2be38`, sidebar host meters with icons) is on `main` under `## Unreleased`, but no release contains it. The user runs the released 0.22.0, so the icons don't show. Releases come from annotated `vX.Y.Z` tags (`.github/workflows/release.yml`), and the version must already be on `main` when the tag is pushed.

**Edits.** Use the same 9 files as the 0.22.0 release (PR #135, merge `e9ce1ee`; see `git show e9ce1ee --stat`):
1. `Cargo.toml:3`: set `version = "0.22.1"`.
2. `Cargo.lock`: run `cargo update -p dogsled --precise 0.22.1 --offline`, or `cargo check`, so the `dogsled` package entry reads 0.22.1. No other lock changes.
3. `package.json:3`, and `package-lock.json` lines 3 and 9: set them to `0.22.1`.
4. `herdr-plugin.toml:5`: set it to `0.22.1`.
5. `plugins/dogsled/.claude-plugin/plugin.json:3` and `plugins/dogsled/.codex-plugin/plugin.json:3`: set them to `0.22.1`.
6. `README.md` lines 14, 20, 28, 35 and 45: change `v0.22.0` / `0.22.0` to `v0.22.1` / `0.22.1`.
7. `CHANGELOG.md`: rename `## Unreleased` to `## 0.22.1 - 2026-10-22`. Keep its one entry as it is.

Find any missed site with: `rg -n '0\.22\.0' -g '!CHANGELOG.md' -g '!plans/**' -g '!target/**'`. It must print nothing when you are done.

**Owned paths.** The 9 files above, and nothing else.

**Acceptance.** `cargo test --locked sidebar::` passes. The `rg` command above prints nothing. `gh pr view <n> --json state` returns `MERGED` once the lead merges it, but you only open the PR.

**Checks.** The pre-commit hook runs `mise run check:rust` and `check:pi`. Let it run, and don't run them a second time by hand.

**Base.** `origin/main` at `4e2be386b8926a40afd4e585918a15339cf6151d`. Use branch `chore/release-0.22.1`. Commit with the message `chore(release): 0.22.1`, push, and open a draft PR. Reply with the PR URL and head SHA. Do not tag. Do not read files outside the owned paths, except `git show e9ce1ee` for reference.
