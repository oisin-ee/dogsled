# Brief: generic CLIProxy quota for plugin providers

## Outcome
`dogsled quota` reports quota for every CLIProxy credential that CLIProxy marks `supports_quota`, through CLIProxy's own normalized quota route. When the planned MiniMax CLIProxy plugin is installed, MiniMax rows appear on the board with no further dogsled change.

## Facts (CLIProxy v8.0.33, source at /tmp/cpa on the lead's laptop; tag v8.0.33 on github.com/router-for-me/CLIProxyAPI)
- `GET /v0/management/auth-files` entries carry `provider`, `auth_index`, `disabled`, and, for credentials served by a quota plugin, `supports_quota: true` and `quota_provider` (`internal/api/handlers/management/auth_files.go` ~line 687).
- `POST /v0/management/quota/fetch` with body `{"auth_index": "<idx>"}` returns `QuotaFetchResponse` (`sdk/pluginapi/types.go` ~1611-1750): `{groups: [{displayName, buckets: [{window, remainingFraction, resetTime (RFC3339), description}]}]}`. Snake-case aliases also exist (`display_name`, `remaining_fraction`, `reset_time`). Errors: 400/404/501/502 with `{error}`.
- Today no live credential has `supports_quota`; claude and codex are not plugin-served and keep their existing `api-call` paths.

## Change (src/quota.rs, README.md, CHANGELOG.md Unreleased)
1. In `collect`, for every enabled auth file with `supports_quota == true`, call `quota/fetch` with its `auth_index` and map each bucket to a row:
   - `pool` = the auth file's `provider`, lowercased (MiniMax will be `minimax`, which pi-tiers already maps from `minimax-*` models).
   - `window` = a slug of `<group displayName>-<bucket window>`, lowercase, `[a-z0-9_-]` only. With a single group, use the bucket window alone.
   - `used_pct` = `(1 - remainingFraction) * 100`, clamped to 0..100. `resets_ms` from `resetTime`.
   - A failed fetch becomes one error row for that account, like the existing per-account errors.
   - Claude and codex credentials keep their current path, even if CLIProxy later marks them `supports_quota`. Do not call both.
2. Remove the `claude-api-key` MiniMax discovery, the `ApiKeys`/`ApiKey` types, and the MiniMax `token_plan/remains` parser. They target a setup that will not exist, and the parser has two known bugs.
3. The snapshot stays version 1 and its JSON shape is unchanged for claude and codex rows. `pool` becomes an open provider string, not a closed enum, for plugin providers. Keep validation of known pools' windows as it is.

## Scope
Owns: `src/quota.rs`, `README.md`, `CHANGELOG.md`. Reads: `src/board.rs`, `pi/`. No new dependencies.

## Checks (writer: light only)
`cargo test quota`, plus `cargo clippy --all-targets -- -D warnings` and `cargo fmt --check`. The commit hook runs the full gate; that counts as the hook. Fix failures in scope or report `blocked`. Never bypass hooks.
Tests (fake transport, as existing): a `supports_quota` credential maps groups and buckets to rows; a failing fetch gives an error row; a disabled credential is skipped; no MiniMax discovery call is made; no secret appears in output.

## Deliver
Draft PR on `feat/quota-plugin-providers`. Report the head SHA, the test counts, and any assumption. Do not merge.
