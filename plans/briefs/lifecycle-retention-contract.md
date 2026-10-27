# lifecycle-retention-contract

Historical architect brief, retained as issued. The resulting review and follow-up decisions supersede its assumptions. Use `plans/lifecycle-cleanup.md` and the current todo contract for new dispatch.

Outcome: Return the smallest safe C8 implementation contract for job expiry, including the replay prerequisites later R1 depends on. No source changes.
Cause: src/cleanup.rs:1892-1910 removes logs and Job JSON together; src/node.rs samples all jobs. src/jobs.rs:294-307 guards replay only while a saved started_ms record exists. work::Request and Launch lack recorded creation timestamps.
Approved policy: terminal job logs and stop markers expire after 24 hours; terminal metadata after 30 days; strict greater-than boundaries. Service timer and explicit prune agree. Running, uncertain and live-referenced records never expire. No owner acknowledgement protocol or new archive store. General finished-store expiry is R1. The host must refuse stale replay without parsing timestamp text from IDs.
Read: plans/lifecycle-cleanup.md, src/jobs.rs, src/cleanup.rs, src/service.rs, src/node.rs, src/work.rs, src/runtime.rs, src/cli.rs, src/main.rs, src/prepare.rs, src/launch.rs, src/unit_request.rs, src/unit_transport.rs, src/items.rs, src/ledger.rs, tests/cli/jobs.rs, tests/cli/launch_retry.rs, tests/cli/stop_sweep.rs.
Trace: request creation to host execution, every wrapper/replay entry, current pending-job persistence, retention readers/locks, live references, snapshot sampling and get/logs/wait callers.
Resolve: how C8 can delete JSON without reopening replay before R1 lands; source and validation of request age; missing legacy timestamps; future timestamps/clock skew; replay of an unresolved request older than 30 days versus the non-expiring prepared-unit contract. Distinguish terminal-record retention from request validity.
Deliver: exact C8 prerequisite and owned-path split (if replay proof must be a separate unit), record fields/protocol boundary with compatibility behavior, atomic deletion sequence, deterministic tests and commands. Show the no-duplicate execution proof after JSON deletion. Return consequential conflicts to the lead; no tombstone forever, ID parsing, unsafe temporary deletion, or new product policy by fiat.
Owned/generated paths: none. No builds, tests, edits, commits, dependency changes, host cleanup or deployment. Report in final reply with source citations.
Base: 4dbbe3de5950d29837f4bfd146c7eb56fc23a0b1. The plan is untracked at /Users/oisin/dev/dogsled/plans/lifecycle-cleanup.md.
Complete your seeded unit and native done protocol. Do not create another task list.
