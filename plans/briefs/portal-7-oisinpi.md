# Brief: pi-todo continues plans approved in Dogsled plan mode (web portal step 7)

**Outcome.** When Dogsled's plan mode approves a plan, pi-todo continues it in the same session. It does this the way it does today for Plannotator, but it listens for `dogsled:plan-approved` instead of `plannotator:plan-approved`.

**Design.**
- Dogsled's `pi/plan-mode.ts` (in `@oisin-ee/pi-dogsled`) emits `pi.events.emit("dogsled:plan-approved", {cwd, planFilePath, planContent, feedback?})`. That is the same payload as Plannotator's event, so `isApprovedPlan` and the handoff logic stay as they are.
- Plannotator is being removed from the dotfiles, so drop the `plannotator:plan-approved` subscription. Do not keep both.

**Edits.**
- **`packages/pi-todo/src/plannotator.ts`:**
  - Rename it to `plan-approved.ts` with `git mv`, and rename the class to `PlanApprovedHandoff`.
  - Subscribe to `dogsled:plan-approved`.
  - Update the continuation text: "approved this plan in Dogsled plan mode". Remove the `[DONE:n]` Plannotator line.
- **`packages/pi-todo/src/plannotator.test.ts`:** rename it to `plan-approved.test.ts` and update it.
- **`packages/pi-todo/src/index.ts`:** update the import.
- **`packages/pi-todo/README.md`:** replace the "Plannotator plans" section (near line 108). It now says that Dogsled plan mode (`/plan` and `submit_plan`) emits `dogsled:plan-approved`, and that no setting is needed. Remove the `plannotator.json` instructions.
- **`packages/pi-guardrails/README.md`, line 22:** planning restrictions belong to Dogsled plan mode, which blocks writes outside markdown files while planning.
- **A changeset:** a minor bump for `@oisin-ee/pi-todo`, and a patch for `@oisin-ee/pi-guardrails`. Follow `.changeset/README.md`.

**Owned paths:** packages/pi-todo/src/plannotator.ts, packages/pi-todo/src/plan-approved.ts, packages/pi-todo/src/plannotator.test.ts, packages/pi-todo/src/plan-approved.test.ts, packages/pi-todo/src/index.ts, packages/pi-todo/README.md, packages/pi-guardrails/README.md, .changeset/.

**Acceptance.**
1. The pi-todo tests and lint pass: `moon run pi-todo:test pi-todo:lint`, or the repo's equivalent from `moon.yml`.
2. `rg -n "plannotator" packages/pi-todo packages/pi-guardrails` prints nothing, apart from CHANGELOG.md.
3. CI on the PR passes.
