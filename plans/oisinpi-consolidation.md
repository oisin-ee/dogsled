# Consolidate oisinpi into Dogsled

Status: approved direction, not started. Facts checked at `origin/main` of each repository: Dogsled `cafbb25` (0.27.0), oisinpi `1c9169d` (0.12.1).

## Goal

Dogsled and oisinpi ship Pi extensions that are installed together and change together.
The split between them costs more than it gives:

- The unit contract types exist three times: `src/unit_transport.rs`, `pi/unit-protocol.ts`, and `oisinpi/crates/todo-core/src/{contract,binding,import}.rs`.
- The `unit-exports.json` test fixture is a copy in each repository. Nothing keeps the copies equal.
- The cross-repository tests (`pi/unit-contract.test.ts`, `pi/work-contract.test.ts`) skip unless `PI_TODO_SOURCE` is set. No CI job sets it.
- About 1,500 lines in `pi/` and 1,570 lines in pi-todo exist only for the event-bus protocol between the two extensions.

The result is one repository, one release, and one Pi extension that owns todos and execution.
Prewalk, tiers, and guardrails move into the repository but stay separate extensions.
Guardrails also serve Claude Code and Codex, so they stay a separate package.

## Steps

1. Copy oisinpi into this repository. No behavior change. This plan specifies this step in full.
2. Make `todo-core` the only owner of the unit contract types. Delete the copies in `src/unit_transport.rs` and `pi/unit-protocol.ts`.
3. Fold pi-todo into the Dogsled extension. Replace the `oisin.tasks.*`, `oisin.todo.*`, and `oisin.work.*` events with direct calls.
4. Release the merged extension and retire `@oisin-ee/pi-todo` and `@oisin-ee/oisinpi`.
5. Port the remaining TypeScript logic to Rust in the native binary, where a port removes risk.

Each step gets its own plan update before work starts.

## Step 1: Copy oisinpi in

Fetch oisinpi, then copy the tracked files of its newest `origin/main` in one commit. Do not import Git history.
Base the branch on the newest Dogsled `origin/main`. Put the oisinpi revision in the commit message.
If either head moved after `cafbb25` or `1c9169d`, repeat the path-collision check and the tool-version table first.
After the copy, apply only the changes that two repositories in one tree need. Do not change runtime behavior.

### Layout

| oisinpi path | Dogsled path |
| --- | --- |
| `crates/*` | `crates/*` |
| `packages/pi-todo`, `pi-prewalk`, `pi-tiers`, `pi-guardrails`, `oisinpi` | `packages/*` |
| `docs/todo-workflows.md` | `docs/todo-workflows.md` |
| `plans/todo-workflows.md`, `PLAN-*.md` | `plans/` |
| `assets/prewalk.png` | `assets/prewalk.png` |
| `.config/proto/plugins/*.toml` | `.config/proto/plugins/` |
| `.oxlintrc.json`, `tsconfig.base.json`, `.npmrc` | repository root |

Dogsled's own extension stays in `pi/` and stays the root package `@oisin-ee/pi-dogsled` until step 3.
No path collides. A check of each copied path against this tree found no existing file.

Do not copy these:

- `.changeset/`, `.github/workflows/publish.yml`, and `scripts/npm-first-publish.sh`. The Dogsled release replaces them.
- The root `package.json`, `Cargo.toml`, `Cargo.lock`, `moon.yml`, `README.md`, `LICENSE`, and `dogsled.toml`. They merge into the Dogsled files.
- The empty `packages/pi-prewalk-kit/` directory. It holds only `node_modules`.

### Tool versions

| Tool | oisinpi | Dogsled | Decision |
| --- | --- | --- | --- |
| Rust | 1.97.1 | 1.99.1 | 1.99.1. Fix new Clippy findings in the moved crates. |
| Node | 26.7.0 | 24 | 26.7.0, pinned exactly. |
| Package manager | pnpm 12.9.1 | npm | pnpm. The packages use catalogs and `workspace:*`. |
| wasm-bindgen | 0.2.129 | 0.2.129 (through Dioxus) | No conflict. Keep the CLI pin in `.prototools`. |
| moon | 2.6.1 | 2.6.1 | No change. |
| shfmt | 3.14.1 | none | Add. The guardrails shell parser needs it. |
| dx | none | 0.7.10, unpinned (`~/.cargo/bin`, CI `cargo binstall`) | Pin `dx = "0.7.10"` through `.config/proto/plugins/dx.toml`, which installs the Dioxus release binary with its checksum. Remove the CI binstall and cache steps. An unpinned `dx` resolves to any proto-installed version, and a global 0.8.0-alpha.3 broke `web-build`. |

The root `package.json` becomes the pnpm workspace root and stays the published `@oisin-ee/pi-dogsled` package.
`pnpm-lock.yaml` replaces `package-lock.json`. The `dogsled.toml` worktree setup uses the oisinpi form: `proto trust .prototools`, then `pnpm install --frozen-lockfile`.

### Cargo workspace

- Add `crates/*` to `[workspace] members`.
- Copy `[workspace.package]`, `[workspace.dependencies]`, and `[workspace.lints]` from oisinpi. Set `rust-version = "1.99"` and the Dogsled repository URL.
- The `dogsled` package keeps its own `[lints]` table. Only the moved crates opt in with `lints.workspace = true`. Thus the stricter oisinpi lints apply only to the moved crates.
- Do not copy the oisinpi `[profile.release]` table. At this root it changes the release profile of the `dogsled` binary, which `[profile.dist]` inherits. That includes `panic = "abort"`.
- Add a `[profile.wasm]` that inherits `release` with the oisinpi values. Build the `*-wasm` crates with `--profile wasm`, and change the moon `wasm` and `generate` paths to `target/wasm32-unknown-unknown/wasm/`.
- `cargo test` and `cargo clippy` at the root still select only the root package. The moved crates run through their moon tasks with `-p`.

### Clippy configuration

Clippy reads the first `clippy.toml` from the crate directory upward ([Clippy configuration](https://doc.rust-lang.org/clippy/configuration.html)).
Put the oisinpi `clippy.toml` at `crates/clippy.toml`. Keep the root `clippy.toml` unchanged.
Without this split, `too-many-lines-threshold = 60` applies to the `dogsled` crate. Its pedantic lints are errors in CI, so functions between 60 and 100 lines fail.
A later decision can adopt the stricter policy for the `dogsled` crate. Measure the findings first.

### moon workspace

- List the moved projects in `.moon/workspace.yml` beside `dogsled: "."`.
- Copy `.moon/tasks/javascript.yml` and `.moon/tasks/rust.yml`. They apply by project tag, and the `dogsled` project has no tag.
- Merge the `implicitInputs` of the two `all.yml` files.
- Move the oisinpi root `format` and `fix` tasks (oxfmt) to the moved paths only. In oisinpi they cover the whole tree, which includes all Dogsled files.
- Change the `dogsled` project tasks `install`, `typecheck`, and `test-shim` from npm to pnpm.
- Set `PI_TODO_SOURCE=$workspaceRoot/packages/pi-todo/src` on `test-shim`, and make it depend on `pi-todo:generate`. Then the contract tests run on every check. The path must be absolute, because `pi/unit-contract.test.ts` loads it from a temporary working directory.

The `dogsled` project keeps `tsc` and `node --test` until step 3. The moved packages keep oxlint and vitest.
Step 3 selects one linter and one test runner for the merged extension.

### CI

Extend `.github/workflows/ci.yml`. Do not copy the oisinpi `ci.yml`.

- The TypeScript job installs every `.prototools` tool, including pnpm, wasm-bindgen, and shfmt. It runs the lint and test tasks of every project.
- The Rust job runs the `clippy` and `test` tasks of the moved crates on both operating systems.
- Keep the pnpm store cache from the oisinpi workflow.

### Release

The tag release (`release.yml` through cargo-dist) publishes every package.
All packages take the Dogsled version. The oisinpi packages go from `0.12.1` to the next Dogsled version after `0.27.0`. The number only increases, so npm accepts it.

- Extend `publish-npm.yml` to build the WASM bindings and the guardrails `dist/`. Then publish each package.
- Check that each `package.json` version matches the tag, and skip a version that is already on npm, for each package.
- Change the `repository` field of each moved package to `oisin-ee/dogsled`.
- Add new entries for the moved packages to the root `CHANGELOG.md`. Keep the package changelogs, but add no new entries to them.

Trusted publishing ties each npm package to one repository and one workflow file.
Before the first release, the npm owner must change the trusted publisher of five packages to `oisin-ee/dogsled` and `release.yml`:
`@oisin-ee/pi-todo`, `@oisin-ee/pi-prewalk`, `@oisin-ee/pi-tiers`, `@oisin-ee/pi-guardrails`, and `@oisin-ee/oisinpi`.
An agent cannot do this step.

### Acceptance

- `moon run :check` and the moved projects' `lint`, `clippy`, and `test` tasks pass at one revision.
- `pi/unit-contract.test.ts` and `pi/work-contract.test.ts` run and do not skip.
- `cargo build --profile dist` has the same profile settings as before. Compare `cargo build --profile dist -v` flags before and after.
- `pnpm pack` for each package gives the same file list as the last published tarball, except `package.json` fields.
- A release from this repository publishes all six packages with provenance.

### After the release

Change local installs only after the release is published on GitHub and npm.
`~/.pi/agent/settings.json` lists `npm:@oisin-ee/oisinpi@0.12.0` and `npm:@oisin-ee/pi-dogsled@0.26.1`. Change both to the new version together.
Then archive the oisinpi GitHub repository. Its README must point to this repository.

## Constraints for steps 3 and 4

- Existing Pi sessions store todo state as custom entries. The merged extension must read the entries of today's pi-todo, with the same `customType` names and payloads. Otherwise open sessions lose their todos.
- Pi rejects a tool or flag that two extensions register. If the merged `pi-dogsled` includes the todo tool, the `oisinpi` collection must drop `pi-todo` in the same release. A host with the old collection and the new extension must not load both.
- `pi/plan-mode.ts` emits `dogsled:plan-approved`, and `packages/pi-todo/src/plan-approved.ts` consumes it. This event becomes a direct call in step 3.
- The seed handoff to a worker (`--todo-unit-file`) crosses a process boundary. It stays a file contract.

## Open items for the owner

- oisinpi has untracked files: `plans/agent-ide-distribution.md`, `plans/agent-ide-go-runbook.md`, and `plans/briefs/`. Step 1 does not copy them. Commit them in oisinpi, or move them here, before the archive.
- Node 26.7.0 replaces Node 24 for Dogsled CI and the publish job. Dogsled `engines` stays `>=22.19.0`.
