# Land the change

1. Run the repository's heavy gate once, at the final revision. Limit it to the change if the task runner can do that. If a target fails, start a follow-up writer unit on the same branch with the failure. A finished writer is closed. When the fix arrives, run only the failed targets again.
2. Run `gh pr ready <n>`, then merge the PR. Then confirm that its state is `MERGED`.
3. Landing never bumps versions or creates tags. Use conventional commit titles for feature PRs because release-please builds the changelog from them. Release only when the user asks: merge the open release-please PR. Its workflow runs the full CI suite, then tags the release commit and dispatches publication. Never auto-merge the release-please PR.
4. If GitHub Actions does not run because of billing, the local gate is the evidence. Say so once in the PR body.
5. Bring the local `main` up to date with `origin/main`. Leave the tree clean.
6. Report to the user what changed and what is live.
