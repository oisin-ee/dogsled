# R1: shared repository resolver and `repo open`

Plan: `plans/repository-transfer.md`, approved 2026-11-10. This brief is complete without the plan.

## Outcome
`dogsled repo open owner/name [--host H]` and MCP `repo_open` return the main checkout of a repository on a host. Leads resolve their checkout through the same function.

## Cause
`src/leads/checkout.rs:29-98` (`matching`, `ensure`) is the only code that finds a main checkout by origin identity or clones one. It is private to leads, and no command exposes it.

## Edits
- `src/checkout.rs`: add `slug_origin(slug) -> Result<String>` with the slug rule from `src/leads/checkout.rs:8-22`. It returns `git@github.com:<slug>.git`.
- `src/checkout.rs`: add `resolve(shell, ledger, home, origin)`. It returns the `Repository` and whether it cloned. Move `matching` and the body of `ensure` here with the same behavior: scan `~/dev` in sorted order, skip linked worktrees, match with `crate::repos::same_origin`, else clone to `~/dev/<name>`, else to `~/dev/<owner>-<name>`, else refuse. Hold one ledger lock across scan and clone. Never return Git stderr. Take name and owner from `identity(origin)`.
- `src/checkout.rs`: add serde struct `Opened` and `open(shell, ledger, home, host_label, slug) -> Result<Opened>`. Fields: `repository` (the `identity`, for example `github.com/owner/name`), `host`, `path`, `branch` (null when detached), `head` (null when unborn), `dirty` (count of paths in `git status --porcelain`), `cloned`.
- `src/leads/checkout.rs`: `validate` calls `slug_origin`. `ensure` calls `crate::checkout::resolve`. Delete the moved code. `src/leads.rs:215` keeps its call.
- `src/runtime.rs`: add `repo_open(shell, ledger, policy, home, host: Option<&str>, slug) -> Result<Value>`. Exemplar: `recovery` at `src/runtime.rs:49`. No host or the local host calls `checkout::open`. Another host calls `fleet.remote_setup(host, ["repo-open", "--repo", slug])`. Validate the slug before transport.
- `src/cli.rs:619`: add `RepoCommand::Open { repo: String, #[arg(long)] host: Option<String> }`. In `HostCommand` (`:728`), add hidden `RepoOpen { #[arg(long)] repo: String }`.
- `src/main.rs`: dispatch `Repo(Open)` without Herdr discovery, as `Command::Recovery` at `:659`. Keep `Repo(List)` at `:222` and `:554`. Add `HostCommand::RepoOpen` to `host` at `:1390`. It prints `checkout::open` as JSON.
- `src/mcp.rs`: add `RepoOpenArgs { repo, host }` with `deny_unknown_fields` and tool `repo_open`. Exemplars: `recovery_list` at `:300`, and `call_bounded(SETUP_LIMIT, …)` in `lead` at `:501`.
- `tests/cli/repo_open.rs` (new, registered in `tests/cli.rs`). In `tests/cli/lead.rs`, make `checkout`, `clone_source`, and the ssh fixture `pub(super)` and reuse them.

Rules: never fetch, switch, reset, or write inside an existing checkout. Make no Herdr call. Change no capacity, queue, or item record. An invalid slug exits 2 before any effect.

## Tests in `tests/cli/repo_open.rs`
1. A matching checkout at `~/dev/elsewhere` with an HTTPS origin is reused. `git.log` has no `clone`. Branch, HEAD, index, and a dirty file are byte-identical. `dirty` is at least 1 and `cloned` is false.
2. A missing checkout is cloned once. A same-name checkout of another owner gives `~/dev/owner-repo`. With both paths occupied, the command fails and changes nothing.
3. `--host netcup` sends exactly one `host repo-open --repo owner/repo` over ssh and clones nothing locally.
4. An invalid slug exits 2 with no git or ssh call.
5. MCP `repo_open` returns the same object as the CLI.

Tests own their child processes, bound every wait, and reap on every path, with the helpers in `tests/cli.rs`.

## Paths
Owned: the files in Edits, plus `tests/cli/mcp.rs` if test 5 needs its helpers. Generated: `web/public/` through `moon run dogsled:web-build`, never committed.
If the change needs another file, stop and reply `blocked` with the evidence. Do not widen scope.

## Acceptance
These pass with nonzero tests: `cargo test --locked --test cli repo_open::`, `cargo test --locked --test cli lead::`, and `cargo test --locked --bin dogsled leads::`. Report the tested SHA.

## Checks and authority
Allowed: `moon run dogsled:web-build` once when `web/public/index.html` is missing (`build.rs` needs it), `cargo fmt --all --check`, and the acceptance commands. `web/public/` is generated and gitignored: never edit, stage, or commit it. The commit hook stays enabled. The lead runs `moon run dogsled:check`.
Commit as `feat(repo): open a repository on any host`. Push only branch `feat/repo-open` to origin. No PR, merge, release, or install.
Reply: branch, full head SHA, changed paths, commands and results, blockers, and unverified scope.

## Base
`origin/main` at `f443a7de94011a4c9f8c38050955a6decc9a78d6`.
