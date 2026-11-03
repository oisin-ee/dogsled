# Surface actionable remote preparation errors end-to-end

## Outcome
A refused remote launch reports its actual safe cause and operation in CLI, agent/MCP tools, persisted admission/ticket state and owner notification, rather than only `remote command failed`.

## Cause and design
At base 263b0f2dfed222d0698d28dc9d39010e2af7f5cf, `src/hosts.rs:358-396` discards peer errors, while `src/error.rs:30-42` deliberately protects against arbitrary peer text. `main.rs:113-117`, `work.rs` Attempt/Ticket, `runtime.rs:74-80`, `unit_request.rs:909-918`, and `mcp.rs:195-197` flatten errors to strings. Repair the diagnostic owner with typed, validated, credential-safe causes; do not expose raw Command argv/stderr or add an opt-in debug workaround.

## Edits
- In `src/error.rs`, own a serializable diagnostic contract: version, safe operation, stable cause code, safe typed context, remote status and request correlation when available; explicitly say when untrusted detail is withheld. Use the existing Error/root structure; keep retry classification intact. Canonical formatting/validation belongs here or one cohesive diagnostic module, not each caller.
- In `src/prepare.rs` and `src/checkout.rs`, raise typed actionable causes at their owners: requested base unavailable, branch already exists with explicit base, occupied workspace, checkout identity mismatch, clone/setup failure. Validate branch/base/workspace context before transporting it; never carry credential-bearing origins, arbitrary script output or task text. Missing-base preflight must not fetch, reset, move a branch, create a worktree, or change existing-branch refusal ordering.
- In `src/main.rs` and `src/hosts.rs`, emit/decode the diagnostic envelope at the host boundary. Keep existing top-level error/deadline/refusal behavior and legacy peer handling, but render known messages locally from validated data. Unknown/malformed envelopes must retain operation/status/correlation with an explicit omission reason, never peer-provided arbitrary messages. Host-health summaries remain categorical.
- Preserve optional diagnostic data through Attempt and Ticket in `src/work.rs`, failed queue waits in `src/runtime.rs`, unit responses in `src/unit_request.rs`, and `src/mcp.rs`. Preserve human-readable errors. Failed owner notifications must name the selected admission host, not `unknown`, and carry actionable causes.
- Remove payload-bearing error context from `hosts.rs` parse failure (currently args.join) and `shell.rs` deadline operation (currently all argv). Report safe operation names without launch JSON, task prompts, scripts or credentials. Preserve timeout/retry/uncertain-start behavior and process cleanup.
- Add `tests/cli/remote_diagnostics.rs`, register it in `tests/cli.rs`, and extend nearby tests for known preparation failures through CLI JSON/text, admission/ticket persistence, owner inbox, unit JSON and MCP. Test malformed/unknown envelopes, secret sentinels, control sequences, payload-bearing parse/deadline cases, and unchanged retry classification. Keep all existing secret-safety assertions. No production launch replay.
- Update only relevant README diagnostic documentation; no changelog/release or automatic synchronization in this unit. Durable preparation-failure replay is a separate dependent unit.

## Owned paths
`src/` (only diagnostic owner and directly required Rust callers), `tests/cli.rs`, `tests/cli/remote_diagnostics.rs`, directly affected `tests/cli/*.rs`, `README.md`. Generated paths: none. Do not edit config, Cargo dependencies, third-party code, Pi TS, unrelated cleanup or live host state.

## Acceptance and checks
Working directory: your isolated Dogsled checkout. Add observable regression tests first where possible; demonstrate a failure at the original seam before repair.
Allowed checks: `cargo fmt --check`; `cargo test --bin dogsled hosts::tests`; `cargo test --bin dogsled shell::tests`; `cargo test --test cli remote_diagnostics::`; `cargo test --test cli launch_retry::`; `cargo test --test cli inbox::`; `cargo test --test cli mcp::`; `cargo test --test cli unit_requests::`; `cargo test --test cli place::`.
Acceptance: `cargo test --test cli remote_diagnostics::` passes, with nonzero coverage of every required caller and failure case; scoped existing checks above pass without weaker assertions. Run formatter when needed. Mandatory pre-commit `mise run check:rust` is authorized once per finished commit; do not bypass hooks or run extra builds/installs/e2e/hosted CI. Report baseline failures separately.

## Base, publication and handoff
Repository: github.com/oisin-ee/dogsled. New branch: fix/remote-actionable-diagnostics. Exact base: 263b0f2dfed222d0698d28dc9d39010e2af7f5cf (origin/main at briefing). Prerequisites: accepted read-only diagnosis; no code prerequisites.
Read project instructions, code-standard repair/language guidance, relevant existing tests, and this brief. Known evidence: three requested commits absent on destinations; a fourth remote request supplied --base for an existing branch. Historic exact errors are unrecoverable; do not claim them proved.
Authority: edit owned sources, run allowed checks, and commit locally. No push, PR, merge, release, deployment, service restart, fetch on live project checkouts, agent closure, or cleanup. Stop and report a genuine blocker or scope conflict; no workarounds.
Reply with branch, full head SHA, changed paths, exact commands/results at that SHA, covered contracts and remaining gaps. Complete the seeded unit, then Dogsled done only when fully finished; never while waiting for input.
