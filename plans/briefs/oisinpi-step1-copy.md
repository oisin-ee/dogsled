# Brief: consolidation step 1, copy oisinpi into Dogsled

**Outcome.** Dogsled holds every oisinpi crate and package, and all checks of both trees pass in one moon workspace. Runtime behavior does not change.

**Design.** `plans/oisinpi-consolidation.md`, section "Step 1: Copy oisinpi in", is the specification. Obey each decision in it: layout, tool versions, Cargo workspace, Clippy configuration, moon workspace, CI, and release. Do not do steps 2 to 5.

**Source.** `git clone --depth 1 https://github.com/oisin-ee/oisinpi.git "$(mktemp -d)/oisinpi"`. Copy its tracked files (`git ls-files`), not `node_modules`, `target`, or untracked files. Record its `HEAD` SHA. If it is not `1c9169d`, or the Dogsled base is not `9006f6c` plus this brief, repeat the collision check and the tool-version table of the plan first.

**Edits.**
- Copy the paths of the plan's Layout table. Skip the paths that the plan says not to copy.
- `Cargo.toml`: add `crates/*` members, the oisinpi `[workspace.package]`, `[workspace.dependencies]`, and `[workspace.lints]`, and a `[profile.wasm]`. The `dogsled` package keeps its own `[lints]`. Do not add a `[profile.release]`.
- `crates/clippy.toml`: the oisinpi `clippy.toml`. Do not change the root `clippy.toml`.
- `packages/pi-todo/moon.yml`, `packages/pi-prewalk/moon.yml`: build with `--profile wasm`. Change `target/wasm32-unknown-unknown/release/` to `target/wasm32-unknown-unknown/wasm/` in `outputs` and in `generate`.
- `.prototools`: add pnpm 12.9.1, wasm-bindgen 0.2.129, shfmt 3.14.1, and the `[plugins.tools]` table. Set node to `26.7.0`.
- `package.json`, `pnpm-workspace.yaml`, `pnpm-lock.yaml`: the root is the pnpm workspace root and stays the published `@oisin-ee/pi-dogsled`. Delete `package-lock.json`.
- `.moon/workspace.yml`, `.moon/toolchains.yml`, `.moon/tasks/`, `moon.yml`: as the plan's "moon workspace" section says. The oisinpi `workspace` project at `.` must not exist beside `dogsled`. Put its `format` and `fix` oxfmt tasks on the `dogsled` project, limited to the moved paths. `test-shim` sets `PI_TODO_SOURCE=$workspaceRoot/packages/pi-todo/src` and depends on `pi-todo:generate`. The path must be absolute: `pi/unit-contract.test.ts:45` loads it from a temporary working directory.
- `.github/workflows/ci.yml`, `.github/workflows/publish-npm.yml`: as the plan's CI and Release sections say.
- Each moved `package.json`: `repository` points to `oisin-ee/dogsled`, with its `directory`.
- `.gitignore`: add the oisinpi entries for generated bindings, `packages/pi-guardrails/dist/`, and `coverage/`. `dogsled.toml`: the worktree setup of the plan.
- `CHANGELOG.md`: one Unreleased entry for the move.
- Rust 1.99.1 can report new Clippy findings in the moved crates. Fix the code. Do not add an `allow` or `expect`.

**Owned paths.** `crates/`, `packages/`, `assets/prewalk.png`, `.config/`, `docs/todo-workflows.md`, `plans/todo-workflows.md`, `plans/PLAN-*.md`, `.oxlintrc.json`, `tsconfig.base.json`, `tsconfig.json`, `.npmrc`, `.prototools`, `.gitignore`, `Cargo.toml`, `package.json`, `pnpm-workspace.yaml`, `moon.yml`, `.moon/`, `.github/workflows/ci.yml`, `.github/workflows/publish-npm.yml`, `dogsled.toml`, `CHANGELOG.md`.

**Generated paths.** `Cargo.lock`: cargo updates it during your Clippy check. Commit it. `pnpm-lock.yaml`: copy the oisinpi file unchanged and delete `package-lock.json`. The lead regenerates `pnpm-lock.yaml` with `pnpm install` during acceptance and commits only that file. `packages/*/src/generated/` and `packages/pi-guardrails/dist/` stay ignored.

**Acceptance.** The lead runs these at the branch head, from the repository root. Do not run them yourself:
1. `moon run :check :lint :test :clippy :format` passes.
2. `moon run todo-core:fmt todo-wasm:fmt prewalk-core:fmt prewalk-wasm:fmt` passes.
3. After `moon run pi-todo:generate`, `PI_TODO_SOURCE="$PWD/packages/pi-todo/src" node --test pi/unit-contract.test.ts pi/work-contract.test.ts` reports `skipped 0`.
4. `cargo build --profile dist -v` gives the same `rustc` flags for the `dogsled` crate as on the base.
5. `pnpm pack --dry-run` for each moved package lists the same files as `npm pack --dry-run <name>@0.12.1`.

**Light checks.** Run only these, once, after the edits:
- `cargo fmt --all --check`
- `cargo clippy -p todo-core -p todo-wasm -p prewalk-core -p prewalk-wasm --all-targets -- -D warnings` (not `--locked`, so that `Cargo.lock` updates)
- `cargo metadata --format-version 1 --no-deps > /dev/null`
- `moon query projects` lists `dogsled` and the eight moved projects, and no `workspace` project.

Do not run `proto install`, `pnpm install`, `npm`, `moon run`, `cargo build`, `cargo test`, `pnpm pack`, or a release.

**Publication.** Commit with `feat(repo): copy oisinpi into the dogsled workspace` and the oisinpi SHA in the body. Push the branch and open a draft PR. Do not merge, tag, or publish.

**Reply.** Branch, head SHA, PR URL, oisinpi SHA, the result of each light check, and any Clippy fixes in the moved crates.

**Base.** `origin/main` at dispatch: `9006f6c` plus the commit that adds this brief.
