# Release Dogsled 0.23.3

## Outcome
Prepare a locally committed, release-ready 0.23.3 on top of the accepted diagnostic and preparation-retention fixes.

## Cause or design
The approved fixes are on local main at 4dbbe3de5950d29837f4bfd146c7eb56fc23a0b1 but not origin/main or any release. Cargo.toml, package.json, all plugin manifests, lockfile root packages, and README install commands still name 0.23.2. The fleet therefore cannot install the fixes from its managed pins.

## Edits
- Set project versions to 0.23.3 in Cargo.toml, package.json, herdr-plugin.toml, plugins/dogsled/.claude-plugin/plugin.json, plugins/dogsled/.codex-plugin/plugin.json.
- Regenerate only the root package versions in Cargo.lock and package-lock.json. Use cargo update --workspace and npm install --package-lock-only --ignore-scripts if needed; preserve all dependency versions and integrity entries. No dependency changes.
- Update README.md installation examples from 0.23.2 to 0.23.3.
- Add CHANGELOG.md entry `## 0.23.3 - 2026-10-23` above 0.23.2. Briefly describe safe actionable remote diagnostics across CLI/MCP/tickets/inbox, retained safe preparation failures and replay, and explicit receipt-write warnings. Do not promise automatic fetch/base repair, historical recovery, or deployment.
- Do not change behavior or tests. Stop and report any unrelated blocker.

## Owned paths
Cargo.toml, Cargo.lock, package.json, package-lock.json, herdr-plugin.toml, plugins/dogsled/.claude-plugin/plugin.json, plugins/dogsled/.codex-plugin/plugin.json, README.md, CHANGELOG.md.
Generated paths: Cargo.lock and package-lock.json, owned by this worker through the existing package managers.
Read first: repository instructions, tests/plugins.rs version-consistency test, README install section, top changelog entries, lefthook.yml, mise.toml, and .github/workflows/release.yml tag trigger plus publish-npm.yml.

## Acceptance
In the worker checkout at final revision: `mise run check`, expected exit 0. Commit hooks stay enabled. Required hook execution counts as the Rust gate; do not manually repeat a full gate already run successfully by the enabled hook at the same tested inputs. Run the Pi gate once if not included by hooks. Record exact commands, exit codes, final SHA, test counts, and any unverified scope.
After commit run `git diff HEAD^..HEAD --check` and confirm only the nine owned files changed, with no dependency upgrades.

## Checks
Allowed: version consistency inspection, `git diff --check`, `cargo update --workspace`, `npm install --package-lock-only --ignore-scripts`, required enabled commit hooks, `mise run check:pi` once, and any missing acceptance target once.
Forbidden: redundant full gates, builds other than check/hook requirements, e2e, host install, service restart, launch/replay, release tag, or dependency changes.

## Base
Repository github.com/oisin-ee/dogsled. Execute on netcup in the assigned linked checkout under ~/dev/dogsled/.worktrees/. The full brief is also at ~/dev/dogsled/plans/briefs/diagnostics-release-rollout.md on netcup.
Branch release/0.23.3 at exact base 4dbbe3de5950d29837f4bfd146c7eb56fc23a0b1. The lead explicitly published/fetched that accepted base before dispatch; do not fetch or substitute another base.
Prerequisite: reviewed diagnostic/retention chain 97b6f42 + 83790a6 + 4dbbe3d, already integrated on local main.
Publication authority: scoped edits and local commit only. No push, PR, merge, tag, npm publish, deployment, or service changes. Lead owns PR/tag publication and managed fleet rollout after release assets and npm are confirmed.
Reply: branch, full final SHA, changed paths, exact verification commands/results, blockers and unverified scope. Complete seeded unit and call done only at final handoff.
