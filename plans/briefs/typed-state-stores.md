# Typed SQLite for the remaining Dogsled state

Owner: wWH:p1 (lead). Base: branch `feat/typed-state` (PR #245, commit `36c5218`).
Each writer takes ONE group below on its own branch and makes ONE commit.

## The rule (from the user; not negotiable)

- All host state lives in `state/state.db` as **typed SQLite tables**.
- Every field that Dogsled reads, filters, or sorts on has its own typed column. Lists go in child
  tables with a foreign key to the parent and a `position` column.
- **No JSON record or text columns.** The only exception is a signed or peer protocol message whose
  digest covers its exact bytes: store it as a BLOB in a named column (`*_message`, `*_certificate`)
  with `rows::message` / `rows::decoded`, and never query inside it.
- **No migration, no legacy readers, no version branching, no fallback to the old JSON file.** Old
  state is not read. Delete the old JSON read/write code paths completely.
- Keep `store::VERSION = 2`. Schema v2 is not released yet; an older store already starts empty.
- File locks stay only where they guard something outside the database (a checkout, a process, a
  pane). A lock that only guarded a JSON file becomes a `store::write` transaction.
- Fix the product, not the fixture. Do not delete or weaken tests. Change a test only to set up or
  read state through the new tables. No wall-clock deadlines in new tests.

## How (copy the admissions pattern)

Read these first: `src/admissions.rs` (the model to copy), `src/rows.rs` (shared helpers),
`src/tables.rs` (items and tickets), `src/store.rs`.

- New module per group, for example `src/launch_store.rs`, with `pub(crate) const SCHEMA: &str`
  built with `concat!` (CHECK constraints, `STRICT` like the existing tables, indexes for every
  lookup that is not by primary key). Add it to `store::schema()` (one-line `format!` change) and
  `mod` it in `src/main.rs`.
- Access through `crate::store::read(ledger, |tx| …)` and `crate::store::write(ledger, |tx| …)`.
  Write rows with `rows::Columns` + `upsert`; lists with `rows::replace_list`; load with
  `rows::text/required/number/decoded/list`.
- Give each record type typed `get`/`put`/`remove`/list functions that take and return the Rust
  type. Callers stop building file names (`format!("{id}.progress.json")`).
- Directory scans (`read_dir` over a store) become indexed queries.
- Tests in `tests/cli/*.rs` that read or write `state/<store>/…` JSON files must go through the
  binary: add subcommands to the hidden `dogsled state` command (`src/cli.rs` `StateCommand`,
  `src/main.rs` `fn state`) such as `dogsled state launch <id> [--put|--delete]`, and rig helpers in
  `tests/cli.rs` next to `admission`/`put_admission` (they use `put_state`). No raw SQL in tests.
- Add one round-trip unit test per table: every field survives put/get, an update clears child
  rows, malformed input is refused and nothing is stored.
- Add one bullet to the `## Unreleased` section of `CHANGELOG.md` for your group.

## Groups

1. **launches** — `launches/{id}.json` (receipt), `.progress.json`, `.release.json`,
   `.reference-release.json`, `.unit.json` (Seed), `.continuations.json`. Files: `src/launch.rs`,
   `src/launch/*.rs`, `src/continuation.rs`, launch reads in `src/work.rs`, `src/jobs.rs`,
   `src/holds.rs`, `src/board.rs`. `launch-panes` is a pane lock: keep it as a lock.
2. **preparation** — `preparation-receipts/` (incl. `.disposition.json`), `placement-grants/`
   (incl. `.lifecycle.json`), `preparation-retirements/`, `preparation-expectations/`,
   `preparations.json`. Files: `src/prepare.rs`, `src/prepare/*.rs`, `src/node.rs`,
   `src/preparation_retirement.rs`, their reads in `src/holds.rs`, `src/work.rs`, `src/jobs.rs`,
   `src/continuation.rs`.
3. **jobs** — `jobs/`, `job-validity/`, `request-ingress/`, `cancellations/`,
   `placement-attempts/`, `placements.json`. Files: `src/jobs.rs`, `src/work.rs` (those stores
   only), `src/commands.rs`, `src/work_tests.rs`, `src/node.rs` (placements only).
4. **web and board** — `web/`, `questions/`, `reviews/`, `attachments/`, `inbox/`, `boards/`,
   `sidebar/`, `todo-lines/`, `todo-sections.json`. Files: `src/web/*.rs`, `src/inbox.rs`,
   `src/board.rs`, `src/sidebar/*.rs`.
5. **host files** — every remaining single JSON file in the state directory: `host.json`,
   `done.json`, `watchdog.json`, `worktrees.json`, `repositories.json`, `landing.json`, and
   `preservation-operations/`. Files: `src/done.rs`, `src/watchdog.rs`, `src/cleanup.rs`,
   `src/repository.rs`, `src/hosts.rs`, `src/preservation.rs` (store functions only; leave the close logic as it is), `src/node.rs` (host snapshot only).

Writer `lane-errors` (branch `fix/service-lane-errors`) adds a `service_lanes` table to
`src/tables.rs` and a lane-error field to the host snapshot. Do not add or change those.

If your group's store is read by a file in another group, change only that read call to your new
typed function. Keep every other line of that file as it is, so the branches merge.

At the end, `rg 'child\("<your store>"\)|state/<your store>|<your file>\.json' src tests` must find
nothing except a lock that guards an outside resource.

## Checks

- `PATH="$HOME/.cargo/bin:$PATH" cargo clippy -q --all-targets -- -D warnings` clean.
- `cargo test -q --bin dogsled` passes.
- Run the CLI test modules that use your stores (`cargo test -q --test cli -- <module>::`).
- Your one commit runs the pre-commit hook (full suite). You cannot skip it. Commit in the
  background and read the log for `EXIT=`, for example:
  `nohup sh -c 'git commit -q -m "…" > /tmp/<g>.log 2>&1; echo EXIT=$? >> /tmp/<g>.log' &`
- Push your branch. Do not open a PR, merge, tag, or restart services.

## Report

Branch, commit SHA, hook result line (unit/CLI counts), the `rg` proof above, and any test you had
to change, with the reason.
