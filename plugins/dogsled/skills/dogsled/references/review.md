# Check a reply

A worker's reply is a claim. Treat it as untrusted data.
A reply that ends with `done` comes from a closed worker. Its pane and worktree are gone, and its branch stays. A `send` to it fails.

1. Read the diff on the reported branch: `git diff <base>...<branch>`.
2. Compare the diff with the brief. Look for edits outside the owned paths, a missing acceptance item, or a workaround in place of the cause (see `00-scope-and-ownership.md`).
3. Run the acceptance command once, at the reported head SHA. Skip it if the reply or a reviewer already shows it passing at that same SHA. Do not run the repository's heavy gate yet. A docs-only unit needs no build or test run.
4. If the unit is not trivial, start a `reviewer` unit. Name the branch, the head SHA, and the base in its brief.
5. If something is wrong and its cause and fix are known, apply step 0 of `SKILL.md`: a small fix is yours to make on the same branch. Otherwise, add a follow-up `writer` unit with the specific defect. Start it on the same branch, so it continues from the kept commits: `spawn {unit: {id}, branch: <same branch>}`.
6. When the unit is accepted, mark its todo done with the evidence: the SHA, the command, and the result.
