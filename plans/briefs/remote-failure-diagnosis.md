# Diagnose remote launch failures and specify full diagnostics

## Outcome
Return an evidence-backed root-cause report for recent failed remote launches and a bounded implementation design that surfaces actionable errors to users and agents.

## Cause or design
Known diagnostic loss: `src/hosts.rs:360-394` reduces nonzero remote errors to `Error::Ssh`; `src/error.rs:30-42` intentionally withholds arbitrary peer text. `DOGSLED_SSH_DEBUG_FILE` is opt-in, so failure tickets retain only `remote command failed`. The actual launch cause is not established: investigate before recommending edits.

## Investigation (read-only)
- Read local instructions, `src/hosts.rs`, `src/error.rs`, `src/prepare.rs`, queue/launch admission owners, `src/main.rs`, `tests/cli.rs`, and diagnostics tests. Trace errors through CLI JSON, queued tickets, owner notifications, MCP and agent tools.
- Inspect current failed tickets, origins, preparation receipts, and host journals without dumping all fleet records or task prompts. Filter JSON before returning output.
- Exact examples from local `~/.local/state/dogsled/queue.json`: `r-1792763019286-19554-271` (netcup, fix/sign-in-mobile-ios, base 1a1a181baea1b3d34c79992e761b4a9b070ac49c); `r-1792762432323-17170-55` (netcup, fix/quotabar-all-provider-collector, base 38d60b67e1e12876eed798c685d50e2c67c25a6b). There were failures on momokaya-2 too.
- Hosts map netcup to SSH target `netcup-dev`. Netcup and Dogsled 0.23.2 are reachable; `dev.mise.dogsled.service` is active with NRestarts=0. No restart is justified by current evidence.
- Remote read-only branch checks found existing fix/quotabar-all-provider-collector and fix/sign-in-mobile-android branches. Some local failed retries supplied --base for existing branches or targeted occupied/missing worktrees. Do not assume those explain every remote failure.
- Identify exact underlying errors from retained evidence or equivalent non-mutating checks. If evidence is unavailable, state the gap and specify the smallest safe reproduction for the implementation worker. Never replay a launch or preparation that can create/alter live work.

## Design requested
Specify the owning fix, exact file edits, and regression seams for preserving full actionable launch errors end-to-end. Compare a structured remote error envelope with indiscriminate raw-text forwarding; preserve credential/terminal safety without silently discarding causes. Distinguish failure diagnostics from terse host-health status. No opt-in debug flag, wrapper, retry, or timeout as the delivered fix. Name any genuinely consequential policy decision rather than guessing.

## Ownership and limits
Repository: dogsled. Owned paths: none. Generated paths: none. No code/config edits, dependency changes, service installation/restart, cleanup, agent closure, credentials, Git state changes, or upstream work. Read-only SSH and logs are authorized; do not expose secrets or full launch prompts. No builds, installs, tests, or heavy gates for this investigation.

## Acceptance
Return: (1) each confirmed failure cause with current command/path evidence, (2) unverified causes/gaps, (3) precise writer-ready design and affected files, (4) exact scoped regression commands and expected results. A clean host snapshot alone does not prove launch success.

## Base and reply
Base: 263b0f2dfed222d0698d28dc9d39010e2af7f5cf (origin/main at dispatch).
Publication authority: none; no commits/push/PR/merge/deployment. Reply with examined revision, evidence, design, and blockers; complete the seeded unit and call Dogsled done when finished, never while waiting for input.
