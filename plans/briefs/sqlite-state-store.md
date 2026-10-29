# sqlite-state-store

**Outcome:** items and queue tickets live in `<state dir>/state.db` (SQLite). No code reads or writes `items.json`, `queue.json`, or `queue-format.json`.

**Design:** `plans/sqlite-state-store.md` on this branch. Read it first. Cause: `items::change` and the other item writers (`src/items.rs:131`, `:161`, `:165`, `:225`, `:302`, `:529`) and `work::save_queue` (`src/work.rs:180`) rewrite and fsync the whole file on every change, and `save_queue` writes two files.
**No legacy handling.** The store starts empty. No import from JSON, no compatibility files, no downgrade path, no old-format tests.

**Edits:**

1. `Cargo.toml`: `rusqlite = { version = "0.41.1", features = ["bundled"] }`. Let cargo update `Cargo.lock`.
2. `src/store.rs` (new; add `mod store;` to `src/main.rs`). It knows nothing about items or tickets.
   - Open `ledger.dir.join("state.db")` (create the dir). `busy_timeout(ledger.budget.remaining("state.db")?)`. Set `journal_mode=WAL` (it returns a row: use `query_row`), `synchronous=FULL`, `foreign_keys=ON`.
   - `user_version` 0: create both tables from the plan's Schema section in one `BEGIN IMMEDIATE` and set it to 1. Above 1: return `Error::Invalid` and change nothing.
   - `read(ledger, |tx| ..)` and `write(ledger, |tx| ..)`; `write` uses `TransactionBehavior::Immediate`.
   - `enum Table { Items, Tickets }` maps to fixed table names. No SQL is built from caller strings.
   - `rows<T: DeserializeOwned>(tx, Table) -> Result<Vec<T>>`, ordered by `seq`.
   - `sync<T: Serialize>(tx, Table, &[T]) -> Result<()>`: match stored rows by `id`; `UPDATE` rows whose `record` text changed, `INSERT` new ids at the end, `DELETE` ids that are gone. Never rewrite an unchanged row.
3. `src/error.rs`: add `Sqlite(#[from] rusqlite::Error)`. `SQLITE_BUSY` and `SQLITE_LOCKED` become `Error::Deadline("state.db busy")`. Remove `Refusal::QueueVersionUnsupported` and every match arm for it.
4. `src/items.rs`: `list` reads `Table::Items`. Every `ledger.write("items.json", &items)` becomes `store::write(.., sync(Items))`. Keep each `with_lock("items")` and every public signature.
5. `src/work.rs`: `queue_records` and `queue` read `Table::Tickets`; `save_queue` syncs `Table::Tickets`. Delete `QUEUE_VERSION`, `QueueEnvelope`, `QueueFormat`, and the format checks (`src/work.rs:133–189`). Keep each `with_lock("queue")`.
6. Tests. Find every site with `git grep -nE 'items\.json|queue\.json|queue-format' -- src tests`.
   - Delete tests that exist only for the removed formats or for old record shapes read from `items.json`/`queue.json`: for example `enqueue_writes_an_array_readable_by_older_servers`, `an_envelope_queue_is_read_and_rewritten_as_an_array`, `a_newer_queue_sidecar_refuses_writes_without_changing_the_files` (`src/work_tests.rs`), `records_from_before_the_request_fields_still_load` (`src/items.rs`).
   - CLI tests: add `Rig` helpers in `tests/cli.rs` next to `read`/`put` (line 949): `items() -> Value`, `put_items(&Value)`, `tickets() -> Value`, `put_tickets(&Value)`. They open `state/state.db` with rusqlite; `put_*` replaces all rows in one transaction and creates the schema if needed. Replace `rig.read("state/items.json")`, `rig.put("state/items.json", ..)` and the queue forms with them. Byte comparisons of these files (for example `IMMUTABLE` in `tests/cli/legacy_continue.rs:249`) compare the helper's JSON instead.
   - Unit tests in `src/`: use `#[cfg(test)]` helpers in `src/store.rs` that do the same.
7. Add tests in `src/store.rs`: a `sync` that changes one record leaves the other rows' `record` and `seq` unchanged; `user_version` 2 refuses; two threads with separate connections writing at once both land.
8. `README.md`: lines 168 and 659 name `state.db` for items and the queue instead of the JSON files.

**Owned paths:** `Cargo.toml`, `Cargo.lock`, `src/store.rs`, `src/main.rs`, `src/error.rs`, `src/items.rs`, `src/work.rs`, `src/work_tests.rs`, `src/work_tests/`, `src/inbox.rs`, `src/sweep.rs`, `src/custody.rs`, `src/loss_tests.rs`, `src/done/tests.rs`, `tests/`, `README.md`.

**Stop and report, do not work around:** a `UNIQUE` failure on `items.id` or `tickets.request_id` in an existing test (do not drop the constraint), or a caller outside the owned paths that reads these files.

**Acceptance:** in the worktree root, `moon run :test :lint :fmt-check` passes (it builds the web bundle the tests need), and `git grep -nE 'items\.json|queue\.json|queue-format' -- src tests` prints nothing.
**Light checks while working:** `cargo clippy --all-targets -- -D warnings`, `cargo test --bin dogsled store::`, `cargo test --test cli <module>::` for the modules you change.
**Base:** `origin/main` `8e9b3f9`, plus the plan commits on `feat/sqlite-state-store`.
**Publication:** commit on `feat/sqlite-state-store` and push that branch. No PR, no merge.
**Reply:** head SHA, changed paths, acceptance output, and anything not done.
