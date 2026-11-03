# Brief: move dogsled repo tooling from mise to proto + moon (web portal step 0)

**Outcome.** The repo's own dev tooling runs on proto + moon: `proto install` provides every tool, moon tasks replace the mise tasks, moon VCS hooks replace lefthook, CI uses moon, and `mise.toml`, `mise.lock`, `lefthook.yml` are gone.

**Design.** Plan: `plans/web-portal.md`, step 0. Follow the working reference setup in `/Users/oisin/dev/momokaya-template` (read-only): `.prototools`, `.moon/workspace.yml` (`vcs.hooks.pre-commit: env -u GIT_DIR moon run :#precommit --affected --status staged`, `vcs.sync: true`, `telemetry: false`), `.moon/toolchains.yml`, root `moon.yml`, `.moon/hooks/`, and `.github/workflows/ci.yml` (`moonrepo/setup-toolchain@261c62cb5b0f580c7be7c8cd0f023a2e96756095 # v0.6.4`, then `proto install`). Docs: https://moonrepo.dev/docs/guides/rust/handbook, https://moonrepo.dev/docs/proto/config. Local versions: proto 0.63.2, moon 2.6.1. Rust stays pinned only in `rust-toolchain.toml` (1.99.0); do not add `rust` to `.prototools`.

Scope is repo tooling only. Dogsled's runtime handling of mise in other repositories (mise trust checks in `src/`, `remote_command = ["mise", "x", ...]`, user install via `mise use`, launchd plists running `mise x`) is product behaviour: do not change it.

**Edits.**
- New `.prototools`: `moon = "2.6.1"`, `node = "24"`; `[settings] unstable-lockfile = true` as in the template if it works with proto 0.63.2.
- New `.moon/workspace.yml`: `versionConstraint: "=2.6.1"`, `projects: { dogsled: "." }`, `vcs.defaultBranch: main`, `vcs.sync: true`, the pre-commit hook above, `telemetry: false`.
- New `.moon/toolchains.yml`: `rust: {}` (uses `rust-toolchain.toml`, no sync of it), `javascript` with `packageManager: npm`, `node`, `proto: { version: "0.63.2" }`.
- New `moon.yml` (`language: rust`), tasks mirroring `mise.toml`:
  - `fmt-check` = `cargo fmt --check`; `lint` = `cargo clippy --all-targets -- -D warnings`; `test` = `cargo test`. Inputs: `src/**/*`, `tests/**/*`, `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`, `rustfmt.toml`, `clippy.toml`, `.cargo/**/*`. Tag `precommit`.
  - `typecheck` = `npm run --silent typecheck`; `test-shim` = `npm run --silent test`. Inputs: `pi/**/*`, `package.json`, `package-lock.json`, `tsconfig.json`. Tag `precommit`.
  - `fmt` = `cargo fmt` (`cache: false`, `runInCI: false`).
- Commit whatever moon generates for hooks under `.moon/hooks/` (as the template does). Add `.moon/cache/` to `.gitignore`.
- Delete `mise.toml`, `mise.lock`, `lefthook.yml`.
- `.github/workflows/ci.yml`: keep both jobs and the rust OS matrix and `Swatinem/rust-cache@v2`; replace tool setup with setup-toolchain + `proto install`; run `moon run :fmt-check :lint :test` (rust job) and `moon run :typecheck :test-shim` (typescript job). Keep `rustup toolchain install` if moon needs it.
- `README.md`: only the development/contributing text (around "The pre-commit hook runs `mise run check`", ~line 657) — describe `proto install`, `moon run :lint` etc., and that moon installs the pre-commit hook. Leave user install and runtime mise text alone.

**Owned paths.** `.prototools`, `.moon/`, `moon.yml`, `mise.toml`, `mise.lock`, `lefthook.yml`, `.gitignore`, `.github/workflows/ci.yml`, `README.md`.

**Acceptance.** From a clean checkout of the branch: `proto install && moon run :fmt-check :lint :test :typecheck :test-shim` exits 0, and `rg -n 'mise run|lefthook' -g '!CHANGELOG.md' -g '!plans/**' -g '!docs/plans/**'` prints nothing. Report the moon output summary and the rg result.

**Checks.** The acceptance command above (it runs fmt, clippy, cargo test, typecheck and shim tests). Do not push CI changes you have not at least validated with `moon ci --help`-level syntax checks; a YAML parse of the workflow is enough locally.

**Base.** `origin/main` at 263b0f2dfed222d0698d28dc9d39010e2af7f5cf.
