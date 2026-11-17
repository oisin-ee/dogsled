# Write a brief

The worker sees only the brief and the contract that its unit seeds. It uses that seeded unit and does not create another. A writer does no planning of its own beyond the brief, so a missing fact becomes a guess or a `blocked` reply.

Write these sections in this order:

1. **Outcome.** One sentence that states what is true when the unit is done.
2. **Cause or question.** For a writer, name the owning cause or design with file paths and line numbers. Resolve missing facts before dispatch. For research, name the unanswered question and the evidence required.
3. **Edits.** For a writer, list the files to change and what changes in each. For repeated edits, give one exemplar and a search command for the other sites. For research, state that the unit is read-only and cannot spawn agents.
4. **Paths.** A writer can change only its owned paths and generated files. A research unit has no owned paths. Name the paths it can read.
5. **Acceptance.** Give the exact command and expected result, or the evidence required for a research result. The command need not be light. The lead does not run it again if it already passed at the same revision.
6. **Checks.** For a research or docs-only unit, name no build or test run. Otherwise, the light checks for this repository, for example `cargo clippy --all-targets -- -D warnings`, a type check of the package, or the unit tests of the changed files. Name them, because the writer runs only these.
7. **Base.** The base commit SHA.

Before you send the brief, check it against the code. Confirm that the paths exist. For a writer, also confirm that the exemplar compiles and no lint or generator rule conflicts with the brief.
Keep the brief under 60 lines. If it needs more, split the unit.
