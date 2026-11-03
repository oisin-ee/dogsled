# Re-review corrected preparation failure retention

## Outcome and revisions
Independently accept or block corrected branch fix/preparation-failure-retention at HEAD 4dbbe3de5950d29837f4bfd146c7eb56fc23a0b1. Corrective parent is 83790a66713b77910b092a8cf03499a34551d10b; complete retention base is 97b6f42ae0ed1a4260a64dfe2f146c7652a8ef7f. Repository github.com/oisin-ee/dogsled. Read-only, no owned/generated paths or edits/commits/publication/deployment/live operations.

## Prior review and blocker
Prior review covered all three retention files, found no other blocker, and blocked only prepare::unretained's untyped Error::Invalid: hosts::parse_remote discarded the receipt-write persistence warning. Prior tests invoked host prepare directly and missed peer transport. Read plans/briefs/preparation-unrecorded-diagnostic.md and plans/briefs/preparation-retention-review.md from the primary checkout.

## Corrective diff and required focus
Corrective changes only src/diagnostic.rs, src/prepare.rs, tests/cli/remote_diagnostics.rs. Lead verified parent, git diff --check, typed OutcomeUnrecorded producer/decoder/renderer and new actual-peer test.
- Confirm the original P2 is fixed at the real boundary: stable outcome_unrecorded cause accepted only for Prepare, fixed message exposes cause was not recorded, caller supplies host/status/request/branch/base, no raw write/source error is carried. Test real_remote_unrecorded_failure_preserves_warning_and_correlation must exercise the actual caller and real peer handler, not just canned envelope/direct-host output.
- Confirm correction preserves retained-failure replay/no side effects, old uncertainty and request conflicts, cleanup/retry semantics and safe context. Review incremental changes plus complete retention behavior in original src/error.rs/src/prepare.rs/tests change for integration safety.
- Check secret sentinels, malformed envelope handling, receipt remains intent-only when outcome write fails, no false persistence success, and failure is visible rather than classified as original setup failure. Phrase inspect this host is acceptable with explicit selected host in caller context; do not request a wording-only follow-up.
- Identify concrete remaining blockers with path/line, reproducible scenario and owning repair. No speculative expansion, base fetch/synchronization, release/deployment or unrelated cleanup.

## Evidence and check limits
Writer reports fail-before on parent (cargo 101: unavailable/missing_envelope), pass-after on HEAD (cargo 0: outcome_unrecorded + safe caller context). Current 21 remote_diagnostics regressions, prepare 3, diagnostic 2, hosts 14 and launch_retry 30 pass; full 316 bin + 274 CLI + 6 plugin tests pass. Enabled commit hook mise run check:rust at HEAD exit0 including fmt/Clippy/full suite. Lead diff check passes. No inputs changed since.
Do not rerun full gates or valid exact-SHA checks. Source review requires no build. If a concrete concern requires an allowed check: one cargo test --test cli remote_diagnostics::real_remote_unrecorded_failure_preserves_warning_and_correlation in exact-SHA isolated checkout; no installs/live hosts/launch replay. Report blocked check limits, never evade hooks.

## Reply
Exact head/parent/base, reviewed paths, explicit PASS or concrete blockers, commands/evidence and limits. Complete seeded unit and Dogsled done only when read-only review is finished.
