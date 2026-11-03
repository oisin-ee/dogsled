# Preserve receipt-write failure across the peer boundary

## Outcome
When preparation fails and its failure receipt cannot be written, the actual remote caller receives an explicit safe `cause was not recorded` diagnostic, not merely `remote command failed`.

## Cause
Independent review blocked 83790a66713b77910b092a8cf03499a34551d10b. `src/prepare.rs:225-229` unretained returns Error::Invalid without diagnostic. `src/hosts.rs:405-437` correctly withholds arbitrary peer error text, so it discards that persistence warning. The new test in tests/cli/remote_diagnostics.rs calls host prepare directly and misses transport. Fix the typed diagnostic owner and local producer, not peer-text forwarding.

## Edits
- In src/diagnostic.rs add a stable typed preparation cause for an unrecorded outcome; allow it in remote()'s Operation::Prepare cause validation, and render a fixed actionable warning: preparation failed, cause/outcome was not recorded, inspect selected host before resubmitting. Keep host/status/request correlation and explicit omission safety. Do not reuse SetupFailed/Unavailable, because that would hide the distinct failure.
- In src/prepare.rs make unretained emit that diagnostic and correlate it to the current request. Carry only safe branch/base/context as appropriate, never raw write error text or discarded original error text in the diagnostic. Wrapping must keep the explicit persistence warning rather than the original preparation cause, and must not claim the failed write succeeded. Retain existing no-replay/uncertain intent semantics and cleanup handling.
- In tests/cli/remote_diagnostics.rs add a faithful caller-side test through existing real_peer_prepare: private setup fails and makes the receipt outcome unwritable; an actual agent/place call through peer parsing must return the new cause and warning, correct host/status/request, no secret/script/path credential payload. Preserve the direct-host test, existing 20 regressions, and replay behavior. Prove the new caller test fails at 83790a6 before repair.
- Add narrow diagnostic-owner assertions only if needed. No other feature/refactor, no automatic fetch, retries, arbitrary peer text, raw log publication or workarounds.

## Owned paths
src/diagnostic.rs, src/prepare.rs, tests/cli/remote_diagnostics.rs. Generated paths none. No config, dependencies, third-party, unrelated source or live host edits. If a correct fix truly requires another owner, stop and report it rather than widening scope silently.

## Acceptance and checks
Isolated child checkout. `cargo test --test cli remote_diagnostics::` must pass existing 20 and the new actual-peer persistence regression. Allowed light checks: cargo fmt --check; cargo test --bin dogsled prepare::; cargo test --bin dogsled hosts::tests; cargo test --test cli remote_diagnostics::; cargo test --test cli launch_retry::. Run formatter if needed. Mandatory enabled pre-commit mise run check:rust authorized once per finished commit; no bypass, extra builds/installs/e2e/hosted CI. Preserve Cargo exit status. Report failures with exact revision.

## Base and authority
Repository github.com/oisin-ee/dogsled. Continue existing branch fix/preparation-failure-retention at 83790a66713b77910b092a8cf03499a34551d10b; omit --base when reusing it. Original retention commit remains preserved and unaccepted until this blocker clears. Prior review found no other blocker in the three changed files.
Read project instructions, code-standard repair guidance and owning producer/decoder/tests before edits. Authority: scoped edits/checks/local commit only, no push/PR/merge/tag/deployment/service restart/live replay/cleanup or third-party change.
Reply full head/parent SHA, paths, exact check results, fail-before/pass-after proof, caller-side persistence warning and uncertainty evidence. Complete seeded follow-up and Dogsled done only when finished and committed.
