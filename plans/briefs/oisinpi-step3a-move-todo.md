# Brief: consolidation step 3a, move pi-todo into pi-dogsled

**Goal.** pi-todo's source lives in `pi/todo/` and ships in `@oisin-ee/pi-dogsled`. `pi/index.ts` registers it. The event bus between the two parts stays. Runtime behavior does not change.

**Spec.** `plans/oisinpi-consolidation.md`, section "Step 3", part "3a decisions". Those decisions are binding.

**Base.** `origin/main` at the commit that adds this brief. Branch `feat/fold-todo`.

**Before you start.** The lead has run `proto install`, `moon run dogsled:web-build`, and `moon run pi-todo:generate` in your worktree. The generated files are gitignored. Do not commit them.

**Edits.**

1. Move `packages/pi-todo/{src,moon.yml,tsconfig.json,README.md,CHANGELOG.md}` to `pi/todo/` with `git mv`. Delete `packages/pi-todo/package.json`.
2. `.moon/workspace.yml`: set `pi-todo: pi/todo`. `pi/todo/moon.yml`: fix the relative paths of the `generate` task (`../../target` becomes `../../../target`). `pi/todo/tsconfig.json`: extend `../../tsconfig.base.json`.
3. `.gitignore` and `.oxlintrc.json` `ignorePatterns`: add `pi/todo/src/generated/`. Root `moon.yml`: add `pi/todo` to the `oxfmt` paths of the `format` and `fix` tasks.
4. `pi/todo/src/source.ts`: `OWN` names `pi/index.ts`, the extension entry, not pi-todo's `index.ts`. Change the tests that depend on it.
5. `pi/index.ts`: import pi-todo's default export from `./todo/src/index.ts`, and call it as the first statement of `dogsled()`.
6. Root `package.json`:
   - `test` runs `node --test "pi/*.test.ts"`.
   - `files` keeps `pi/` and excludes `pi/**/*.test.ts`, `pi/todo/moon.yml`, and `pi/todo/tsconfig.json`. If `pnpm pack` drops `pi/todo/src/generated/`, the lead tells you to list it.
   - Add peer dependencies `"@earendil-works/pi-ai": "*"` and `"typebox": "*"`, and dev dependency `"typebox": "catalog:"`.
7. Root `tsconfig.json`: add `"exclude": ["pi/todo/**"]`.
8. Root `moon.yml` `test-shim`: delete the `PI_TODO_SOURCE` env. Keep `deps: [pi-todo:generate]`. Add `pi/todo/**/*` to the `typecheck` and `test-shim` inputs only if a test in `pi/` imports from it.
9. `pi/unit-contract.test.ts` and `pi/work-contract.test.ts`: import from `./todo/src/` directly. Delete the `PI_TODO_SOURCE` lookup and its skip.
10. New `pi/todo-registration.test.ts` (`node --test`): load `pi/index.ts` with a fake Pi API. Assert that `todo` is registered exactly once, and that one `oisin.tasks.acquire.v1` emit gets exactly one provider reply.
11. `packages/oisinpi/package.json`: remove `@oisin-ee/pi-todo` from `dependencies`, `bundleDependencies`, and `pi.extensions`. `packages/oisinpi/README.md`: say the `todo` tool now ships in `@oisin-ee/pi-dogsled`.
12. `.github/workflows/publish-npm.yml`: remove `packages/pi-todo` from the package loop. Delete the `PI_TODO_SOURCE` env.
13. `tests/plugins.rs`: remove `packages/pi-todo/package.json` from the version-sync list.
14. Path references only, no other prose changes: `docs/todo-workflows.md`, `docs/unit-requests.md` (the `PI_TODO_SOURCE` example), `docs/pi-work.md` (the `PI_TODO_SOURCE` example), and the `description` in `crates/todo-wasm/Cargo.toml`.

**Lockfile.** You may not run an install. After edits 1, 6, and 11, send the lead `blocked: lockfile`. The lead runs `pnpm install` in your worktree and replies. Then continue.

**Owned paths.** `pi/`, `packages/pi-todo/`, `packages/oisinpi/`, `package.json`, `pnpm-lock.yaml`, `tsconfig.json`, `moon.yml`, `.moon/workspace.yml`, `.gitignore`, `.oxlintrc.json`, `.github/workflows/publish-npm.yml`, `tests/plugins.rs`, `docs/todo-workflows.md`, `docs/unit-requests.md`, `docs/pi-work.md`, `crates/todo-wasm/Cargo.toml`.

**Not owned.** `src/`, `crates/todo-core/`, `packages/pi-prewalk/`, `packages/pi-tiers/`, `packages/pi-guardrails/`, `plans/`. If a change there seems necessary, stop and report.

**Stop and report** if the moved code needs a behavior change to pass, other than edit 4.

**Your checks.** `moon run pi-todo:lint pi-todo:test dogsled:typecheck dogsled:test-shim dogsled:format`. Do not run the full suite, a release, or a publish. The lead runs the full acceptance.

**Publication.** Commit with `refactor(pi): move pi-todo into the Dogsled extension` through the normal hook. Push the branch and open a draft PR. Do not merge, tag, or publish.
