# Review preparation failure retention

## Outcome and revision
Independently review branch fix/preparation-failure-retention at exact HEAD 83790a66713b77910b092a8cf03499a34551d10b against base 97b6f42ae0ed1a4260a64dfe2f146c7652a8ef7f. Return concrete blockers or explicit PASS. Repository github.com/oisin-ee/dogsled; all review is read-only. No owned/generated paths, edits, commits, installs, builds, full gates, service changes or live launch replay.

## Contract and changed paths
Read plans/briefs/preparation-failure-retention.md from the primary checkout. Changed paths: src/error.rs, src/prepare.rs, tests/cli/remote_diagnostics.rs. The outcome must distinguish durably returned preparation failure from interruption, replay original safe cause without preparation side effects, preserve legacy null uncertainty/success/request-conflict and classification, and explicitly report outcome persistence failure.

## Inspect
- Receipt state transitions, matching request tuple, contradictory receipt handling, success replay and failure replay under changed host state. No original failure may rerun worktree/setup/Herdr.
- RetainedFailure serialization/reconstruction for diagnostics and root/refusal/deadline/transport classification. Check defaults, malformed state, secret safety and correlation against existing Diagnostic owner; no raw text/argv/stderr/scripts/origins.
- Pending cleanup holds and uncertain-start semantics must remain correct. Report scope creep or weakened tests.
- Lead concern to investigate, not a pre-decided finding: prepare::unretained returns only Error::Invalid and no diagnostic. The new CLI test invokes `host prepare` locally, while hosts::parse_remote discards arbitrary remote error strings. Does a real caller through the peer boundary actually receive the explicit persistence-failure reason, or is it again only `remote command failed`? Worker claims no diagnostic keeps the failure visible in peer mode; verify that claim through code or a narrowly scoped real-peer regression. Likewise assess safe retained untyped failures and contradictory receipts through remote replay, not only direct host output.

## Evidence and limits
Writer reports exact-SHA enabled-hook mise run check:rust passed fmt, Clippy -D warnings, and full 314 bin + 273 CLI + 6 plugin tests. Scoped remote_diagnostics:: passed 20 (13 existing + 7 new); prepare:: 3, place:: 4, launch_retry:: 30. Reported three failing-before regressions. Lead verified scoped stat/diff check and inspected receipt/Error implementation. Do not repeat valid full gates.
Allowed only if needed to resolve concrete review uncertainty: one scoped `cargo test --test cli remote_diagnostics::<specific_test>` on an exact-SHA isolated checkout or existing disposable-fixture executable; preserve live state. No new files in repository or source edits; if faithful reproduction requires edits, report the exact missing regression and source reasoning instead. If harness blocks shell, do not evade it.

## Reply
Return exact SHA/base, reviewed files, explicit verdict, concrete path/line/inputs/impact/owning repair for each blocker, checks executed and gaps. Complete seeded unit and call Dogsled done only when finished. Do not demand automatic base synchronization or deployment; those are outside this unit.
