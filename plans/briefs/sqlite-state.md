# All Dogsled state in typed SQLite: design brief

Base: origin/main 571462b (v0.29.0). Read-only design. Return the design as your final reply.

## User order (binding)
- All Dogsled host state lives in one SQLite database, `state/state.db`.
- Use typed columns. Do not store JSON records or blobs in columns. Nested data gets its own tables with foreign keys. Use a column only for an opaque payload that Dogsled never queries, such as a transcript chunk or a provider event body. Name each such column and justify it.
- Start clean. Nothing reads, imports, migrates or deletes old JSON state. There is no legacy reader, no compatibility path and no version branching for old records. Old files are simply ignored.
- Per-request lock files and `Ledger::with_lock` mutual exclusion become SQLite transactions where the lock only guards state. Keep a file lock only where it guards something outside the database, such as a Git checkout or a process.

## Current state to replace
Ledger children (one JSON file per record) and ledger JSON files. Find every one with:
`rg -o 'child\("[a-z-]+"\)' src` and `rg -o '"[a-z-]+\.json"' src`.
They include admissions, launches, cancellations, jobs, job-validity, placement-attempts, placement-grants, preparation-receipts, preparation-expectations, preparation-retirements, preservation-operations, request-ingress, inbox, questions, reviews, attachments, todo-lines, launch-panes, custody, done, gate, host, landing, orphans, outbox, parents, placements, preparations, worktrees and watchdog. Items and tickets are already in `src/store.rs`, but as JSON `record` columns, so they must be redone as typed tables too.
Live data on one host: 7.7k admission files (33 MB), 6.2k launch files (489 MB), 4k placement attempts and an 8 MB queue. `dogsled unit settlements` scans whole directories on every call and burns about 11 cores across Pi sessions.

## Scope note
Logs and transcripts that are append-only text streams may stay as files if they are only ever read as text. Say which ones, and why.

## Return
1. A schema: every table, its typed columns, primary and foreign keys, and the indexes the hot paths need. Hot paths: settlements by owner and unsettled state; admission; placement; refresh by host; the board and status; retention expiry.
2. The store API shape in Rust: typed row structs and functions per domain, and a transaction style that replaces the per-record locks.
3. How the cross-host protocol (host.json snapshots, remote `host ...` commands) keeps its wire format while the host stores rows.
4. Writer units: split by domain so units can run in parallel without editing the same files. For each unit give the owned files, its deliverable, and acceptance tests. Make the schema and store API the first unit, which the others depend on.
5. A list of every test helper and fixture that writes state files, with the rig helper that replaces it (for example `rig.put("state/...json")`).

Constraints: read-only. No edits, no builds, no full test runs. Run `git fetch origin` first and read origin/main. Keep the reply under 400 lines.
