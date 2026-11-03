# Review actionable remote diagnostics at exact revision

## Outcome
Return an independent correctness/security/contract verdict for the diagnostic repair before it can be accepted or integrated.

## Revision and scope
Repository github.com/oisin-ee/dogsled. Review branch fix/remote-actionable-diagnostics, exact HEAD 97b6f42ae0ed1a4260a64dfe2f146c7652a8ef7f, parent/base 263b0f2dfed222d0698d28dc9d39010e2af7f5cf. Read plans/briefs/remote-diagnostics-implementation.md in the primary checkout. This is read-only: no edits, commits, push, PR, merge, cleanup, installs, service operations or live launch replay. Owned paths none; generated paths none.

## Known cause and implementation
The old host boundary discarded preparation failures as remote command failed. Current remote checks showed three absent requested base objects and one existing branch plus --base refusal. Historic exact errors were not retained. The repair adds src/diagnostic.rs and Error::Diagnostic, typed preparation/checkout causes, safe operation descriptions, remote encoding/validation, Attempt/Ticket persistence and CLI/unit/MCP/inbox propagation. Retaining failure outcomes in preparation receipts is a separately tracked dependent unit; automatic base synchronization is not authorized here.

## Review focus
- Verify typed contexts are caller-bound at the peer boundary, safe for credentials/control sequences, and not arbitrary peer strings disguised as structured diagnostics. Check unknown/malformed envelopes and legacy refusal/host-health behavior.
- Trace cause/host/status/request through synchronous launch, queue/admission, failed waits, owner notices, unit JSON/observe and MCP. Ensure wrappers preserve root classification, retry decisions, uncertain starts and startup evidence; no payload-bearing parse/deadline strings remain in this path.
- Inspect preflight ordering/behavior (existing-branch+base refusal before missing-base check), checkout trust preservation, setup failure and cleanup semantics. Check whether generic classifications still silently mask relevant actionable failures or mislabel them.
- Inspect tests against real seams, including new 13-case tests/cli/remote_diagnostics.rs. Do not accept weaker legacy assertions as proof. Evaluate every changed file and scope against the implementation brief. Do not demand deferred receipt retention/base synchronization in this unit.
- Report concrete blocking defects with path/line, reproducing inputs, impact, and owning fix. Separate suggestions, assumptions, and gaps. If no blocker exists, give an explicit pass verdict at the exact SHA.

## Evidence and check limits
Writer reports enabled-hook mise run check:rust passed at HEAD: cargo fmt --check, cargo clippy --all-targets -- -D warnings, cargo test (311 binary + 266 CLI + 6 plugins = 583). Explicit cargo test --test cli remote_diagnostics:: passed 13. Regression failed at original boundary before repair. Lead verified 17 changed files and git diff --check.
Do not repeat valid full gates. Source review needs no build. If a concrete uncertainty requires a check, allowed: cargo test --test cli remote_diagnostics::<specific_test> in an exact-SHA isolated checkout, no live state; report it precisely. No other heavy checks/install/hosted CI. If the read-only hook blocks shell access, use read tools and report that check limit; never evade it.

## Reply
Return exact examined head/base, files covered, verdict and findings, evidence used/run, and unverified scope. Complete the seeded review unit and call Dogsled done only when the read-only review is finished.
