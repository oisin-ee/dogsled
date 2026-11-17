# Land the change

1. Do not run the heavy gate to land a change. Heavy checks (full builds, full test suites, acceptance) run only before a release. Confirm that the cheap checks passed on the host at the final revision, and that you ran the changed functionality on the real system and saw it work. Work is only completed when it is merged to main.
2. Run `gh pr ready <n>`, then merge the PR. Then confirm that its state is `MERGED`.
3. Landing never bumps versions or creates tags. Use conventional commit titles for feature PRs because release-please builds the changelog from them. Release only when the user asks: merge the open release-please PR. Its workflow reuses the passing CI proof of the tree under the release commit, or runs the full CI suite on the release commit when no run proved that tree. Then it tags the release commit and dispatches publication. A failed release stays outstanding: retry it by dispatching `release-please.yml` before anything else lands on main, or abandon it by reverting its release commit with a `chore(release):` subject. Never auto-merge the release-please PR.
4. Pull requests start no hosted CI. The release workflow runs the full CI suite on the release commit before it tags the release.
5. Bring the local `main` up to date with `origin/main`. Leave the tree clean.
6. Report to the user what changed and what is live.
