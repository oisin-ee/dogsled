# Brief: canonical repository identity on probe agents and board rows (web portal step 1)

**Outcome.** Every probe agent pane and every board row carries `repository: Option<String>` in the canonical `host/owner/repo` form (for example `github.com/oisin-ee/rondo`), identical across hosts and across linked worktrees of one repo.

**Cause / design.** Plan: `plans/web-portal.md`, step 1. Today rows carry only host paths (`src/board.rs` `Row`, line ~440; `src/probe.rs` `Agent`, line ~241, has `cwd` and `worktree`). The canonical rule exists only in TypeScript: `repositoryIdentity` in `pi/unit-protocol.ts:77` (scp form, ssh/https/http/git URLs, strip `.git` and slashes, lowercase host, keep path case, reject local paths, empty/dot segments, query or fragment). The origin lookup already exists in `checkout::discover` (`src/checkout.rs`, `git remote get-url origin` on `prepare::main_checkout`). Per-cwd facts are already observed once per tick in `facts::observe` (`src/facts.rs:188`), which is where the repository belongs.

**Edits.**
- `src/checkout.rs`: add `pub fn identity(origin: &str) -> Result<String>`, a byte-for-byte port of `repositoryIdentity`, errors as `Error::Invalid`. Add a small helper that returns the origin for a cwd (reuse the existing `git -C <main checkout> remote get-url origin` code path; do not duplicate it).
- New `tests/fixtures/repository-identity.json`: `{ "valid": [[origin, identity], ...], "invalid": [origin, ...] }`. Cover scp `git@github.com:owner/repo.git`, `https://github.com/owner/repo.git`, `ssh://git@github.com/owner/repo`, mixed-case host, trailing slash, `/tmp/repo`, `file:///x`, `https://h/a/../b`, a URL with `?q`.
- Rust unit test in `src/checkout.rs` reads that fixture (`include_str!`). `pi/unit-bridge.test.ts` (test near line 107) reads the same fixture and asserts the same results, replacing its inline cases.
- `src/probe.rs`: add `#[serde(default, skip_serializing_if = "Option::is_none")] pub repository: Option<String>` to `Agent`.
- `src/facts.rs` `observe`: compute the repository once per distinct cwd in the same threaded pass as `worktree`, reuse `previous` when the agent's cwd is unchanged, and set `agent.repository`. A cwd outside a git repo, or an origin that fails `identity`, gives `None` (never an error for the whole probe).
- `src/board.rs`: add `#[serde(skip_serializing_if = "Option::is_none")] pub repository: Option<String>` to `Row`; fill it in `agent_row` from the matched agent, in `job_row` from the job's cwd only if a matching agent pane with that cwd has one (otherwise `None`), `None` in `waiting`. Update test constructors.
- Keep the sidebar (`src/sidebar/`) compiling; do not change its display.

**Owned paths.** `src/checkout.rs`, `src/probe.rs`, `src/facts.rs`, `src/board.rs`, `src/sidebar/model.rs` (constructor fixes only), `tests/fixtures/repository-identity.json`, `pi/unit-bridge.test.ts`.

**Acceptance.** `cargo test` and `npm run --silent test` both pass, including the new fixture-driven identity tests in Rust and TypeScript, and a `facts`/`board` test that shows two agents in different worktrees of one repo get the same `repository` and that `Row.repository` serialises in `board::build` JSON.

**Checks.** `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`, `npm run --silent typecheck`, `npm run --silent test`. (Step 0 moves these into moon in parallel; use the cargo/npm commands directly.)

**Base.** `origin/main` at 263b0f2dfed222d0698d28dc9d39010e2af7f5cf.
