# Retain returned preparation failures

## Outcome
A completed preparation failure is durably retained and replays its original safe cause without rerunning side effects; an interrupted preparation remains explicitly uncertain.

## Cause and design
At exact base 97b6f42ae0ed1a4260a64dfe2f146c7652a8ef7f, `src/prepare.rs:133-174` writes intent and success only; `local_once(...)?` leaves every returned failure indistinguishable from interruption. The safe Diagnostic contract now exists in `src/diagnostic.rs` and Error::Diagnostic/root in `src/error.rs`. Retain failure outcomes at the preparation receipt owner, without duplicating the admission/ticket store or forwarding arbitrary source error text.

## Edits
- Extend `PreparationReceipt` in `src/prepare.rs` to distinguish incomplete, successful and returned-failure outcomes. Preserve existing serialized `prepared` records, including old `prepared:null` as uncertain; reject contradictory success/failure state. Use one safe stored failure representation and no parallel task database.
- After intent is durable, match the result of `local_once`; persist either success or safe returned failure before returning it. A failure writing the outcome must not claim that the original outcome is retained. Keep the uncertain intent and report the persistence failure explicitly.
- Replay only the identical request tuple: return success as before, reconstruct the retained safe failure without `local_once` or any worktree/setup/Herdr side effects, and keep request-change conflict checks. Never infer historical failure/success for old null receipts.
- Retain the canonical typed Diagnostic cause/context/request from the prerequisite and enough safe classification to preserve meaningful root/refusal/deadline behavior. If an error has no typed diagnostic, persist safe operation/correlation and an explicit withheld-detail marker; never serialize Command argv/stderr, prompts, scripts, credential-bearing origins, or raw Error::Invalid/Io/Parse text. Put any shared safe reconstruction API in `src/error.rs` or `src/diagnostic.rs`, not duplicated at callers. Keep incomplete/uncertain-start semantics distinct from recorded failure.
- Preserve existing pending cleanup-hold release and preparation cleanup semantics. Do not change retries, base synchronization, branch reuse policy, repository setup ownership, or any live host/agent state.
- Add regressions in `tests/cli/remote_diagnostics.rs` (and private tests inside the preparation owner if needed) for identical failed request replay after host state changes, request tuple conflict, successful replay, old null interruption, classified refusal/deadline, unknown-secret-safe failure, contradictory receipt rejection, and outcome-persistence failure. Use disposable repositories/fake Shell; no production launch replay. Side-effect logs must prove failure replay never executes preparation again.

## Owned paths
`src/prepare.rs`, `src/error.rs`, directly necessary `src/diagnostic.rs`, `tests/cli/remote_diagnostics.rs`. Generated paths: none. No configuration, dependencies, unrelated tests, Pi TS, third-party or documentation edits.

## Acceptance and checks
Working directory: isolated Dogsled child checkout. Acceptance: `cargo test --test cli remote_diagnostics::` passes existing 13 plus new retention cases, proving original safe failure replay with no preparation side effects and unchanged legacy uncertainty/success/conflict semantics.
Allowed light checks: `cargo fmt --check`, `cargo test --bin dogsled prepare::`, `cargo test --test cli remote_diagnostics::`, `cargo test --test cli place::`, `cargo test --test cli launch_retry::`. Run formatter when needed. Mandatory enabled pre-commit `mise run check:rust` is authorized once per finished commit; no hook bypass, extra builds, installs, e2e or hosted CI. Report pre-existing failures and check limits separately. Add a faithful regression that fails before the repair where practical.

## Base, authority and reply
Repository github.com/oisin-ee/dogsled. New branch fix/preparation-failure-retention. Exact base 97b6f42ae0ed1a4260a64dfe2f146c7652a8ef7f, now integrated into local main after independent PASS. Prerequisite unit u-7ba5bf85-fa3a-4d26-9e2a-3e6d84154187-6 accepted with 583-test enabled-hook proof.
Read this brief, project instructions, existing receipt/cleanup owner and Diagnostic/Error interfaces before editing. Authority: scoped edit/check/local commit only. No push, PR, merge, deployment, service restart, cleanup, live project fetch or third-party changes. Stop with evidence for a genuine blocker, never a workaround.
Reply with branch/head/base SHAs, paths, exact command/results at final SHA, replay/classification/secret-safety coverage and gaps. Complete the seeded unit and call Dogsled done only when committed and finished, never while waiting for input.
