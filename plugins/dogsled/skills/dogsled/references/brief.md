# Write a brief

The worker sees only the brief. A writer does no planning of its own beyond the brief, so a missing fact becomes a guess or a `blocked` reply.

Write these sections in this order:

1. **Outcome.** One sentence that states what is true when the unit is done.
2. **Cause or design.** Name the owning cause and where it lives, with file paths and line numbers. If you cannot write this sentence, spawn an `architect` first.
3. **Edits.** List the files to change and what changes in each. For the same edit at many sites, give one exemplar and say how to find the other sites, for example with a search command.
4. **Owned paths.** The worker may change only these paths and the files that they generate.
5. **Acceptance.** The exact command that proves the outcome, and its expected result.
6. **Checks.** The light checks for this repository, for example `cargo clippy --all-targets -- -D warnings`, a type check of the package, or the unit tests of the changed files. Name them, because the writer runs only these.
7. **Base.** The base commit SHA.

Before you send the brief, check it against the code. Confirm that the paths exist, that the exemplar compiles, and that no lint or generator rule conflicts with the brief.
Keep the brief under 60 lines. If it needs more, split the unit.
