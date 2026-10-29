# Move items and the queue to a per-host SQLite store

Status: proposed. Not approved for dispatch. Facts checked on 2026-10-29 at Dogsled `origin/main` `f208ce3`. Line numbers refer to that revision.

## Goal

Each host keeps its items and its queue in one SQLite database in its state directory.
A change writes only the rows that changed, in one transaction.
The cross-host protocol, the kernel locks that guard each operation, and every other store stay as they are.

## Why

- **Whole-file rewrites.** Each item change reads, rewrites, and fsyncs all of `items.json` (`src/items.rs:127` `change`, and 5 other write sites in the same file). Each ticket change does the same for `queue.json` (`src/work.rs:180` `save_queue`). On the local host these files are 4.0 MB and 7.9 MB (measured 2026-10-29). The cost per change has not been measured. Step 0 measures it.
- **Two files for one fact.** `save_queue` writes `queue-format.json`, then `queue.json`, as two separate atomic renames (`src/work.rs:180–189`). A crash between them leaves the two files out of step.
- **The storage is already well bounded.** Only `src/items.rs` and `src/work.rs` read or write these files in product code. Every `with_lock("queue")` is in `src/work.rs`. The one `with_lock("items")` outside `src/items.rs` is `src/inbox.rs:1214`. So the change can stay inside two owners.

## Non-goals

- No change to the cross-host protocol, to request IDs, or to the ownership rules in `src/node.rs`.
- No removal of kernel locks. Some critical sections do an external action under the lock. For example, `notify_child` notifies the custodian while it holds the custody and `items` locks (`src/inbox.rs:1204–1218`). A SQLite transaction must not span that, so the kernel lock stays the guard and the transaction covers only the write.
- No other stores in this plan: outbox, custody, notification failures, launches, admissions, evidence, job records, logs, Git refs, and configuration stay as they are.
- No retention. Pruning finished items and tickets is unit R1 in `plans/lifecycle-cleanup.md` (section 7). It is still open. See "Order with R1".

## Design

### One database per host

- Path: `<state dir>/state.db`, where the state dir is `Ledger::dir` (`src/ledger.rs:50` `default_dir`). `Ledger::child` sub-ledgers do not get their own database.
- Open one connection per store operation. Apply these settings on each open: `journal_mode=WAL`, `synchronous=FULL`, `foreign_keys=ON`, and a busy timeout equal to the remaining `Budget` (`src/budget.rs`), so a busy database gives the same `Error::Deadline` as a busy kernel lock.
- Writes use `BEGIN IMMEDIATE` ([SQLite transactions](https://sqlite.org/lang_transaction.html)). Reads use plain read transactions. Under WAL, readers do not block the writer.
- The state directory must be on a local disk. WAL does not work over a network filesystem ([WAL](https://sqlite.org/wal.html)).
- `PRAGMA user_version` holds the schema version. A binary that finds a newer version refuses every store operation, with the same `Refusal::QueueVersionUnsupported` style of error used now (`src/work.rs:149–178`). This replaces the `queue-format.json` sidecar.

### Schema v1

Each table keeps the full serde record as JSON text, so field defaults, `skip_serializing_if`, and old-record compatibility stay where they are now: on the Rust types. The columns that queries and uniqueness need are generated from that JSON, so they cannot drift from the record ([generated columns](https://sqlite.org/gencol.html)).

```sql
CREATE TABLE items (
  seq        INTEGER PRIMARY KEY,               -- list order, as in items.json
  record     TEXT NOT NULL CHECK (json_valid(record)),
  id         TEXT GENERATED ALWAYS AS (json_extract(record, '$.id')) STORED NOT NULL UNIQUE,
  host       TEXT GENERATED ALWAYS AS (json_extract(record, '$.host')) STORED NOT NULL,
  pane       TEXT GENERATED ALWAYS AS (json_extract(record, '$.pane')) STORED NOT NULL,
  closed_ms  INTEGER GENERATED ALWAYS AS (json_extract(record, '$.closed_ms')) STORED
);
CREATE INDEX items_open_pane ON items (host, pane) WHERE closed_ms IS NULL;

CREATE TABLE tickets (
  seq         INTEGER PRIMARY KEY,              -- queue order, as in queue.json
  record      TEXT NOT NULL CHECK (json_valid(record)),
  id          TEXT GENERATED ALWAYS AS (json_extract(record, '$.id')) STORED NOT NULL UNIQUE,
  request_id  TEXT GENERATED ALWAYS AS (json_extract(record, '$.request.id')) STORED NOT NULL UNIQUE,
  state       TEXT GENERATED ALWAYS AS (json_extract(record, '$.state')) STORED NOT NULL
);

CREATE TABLE migrations (
  collection  TEXT PRIMARY KEY,                 -- 'items' or 'queue'
  state       TEXT NOT NULL CHECK (state IN ('imported', 'sealed')),
  source_sha256 TEXT NOT NULL,                  -- the JSON file that was imported
  at_ms       INTEGER NOT NULL
);
```

The writer confirms before coding that existing data has no duplicate item IDs or request IDs. `record` (`src/items.rs:218`) and `enqueue` (`src/work.rs:379`) refuse duplicates today, so a duplicate in a real file is a finding to report, not something to drop silently.

### Store API

A new owner, `src/store.rs`, opens the database, applies the settings and migrations, and gives `read` and `write` closures over a `rusqlite::Transaction`. It knows nothing about items or tickets.

`src/items.rs` and `src/work.rs` keep their public functions and signatures. Only their bodies change:

- `list` and `queue` read all rows in `seq` order and decode each `record`.
- `change`, `record`, `close`, `set_reply_with`, `reconcile`, `save_queue`, and the other writers keep their kernel lock. Inside it they load the rows, apply the same closure, and in one transaction insert new records, update only records whose JSON changed, and delete removed ones (only `expire_requests` removes tickets today, `src/work.rs:225`).
- `queue_records` keeps returning raw JSON values for its callers (`src/work.rs:347`, `:1496`). The array/envelope parsing moves into the import.

Reads still decode the whole collection, as now. Indexed point reads are a later change, made when a caller needs one.

### Migration and cutover

Each collection migrates on its own, under its own kernel lock. No step holds both `items` and `queue`, so the migration adds no new lock order.

1. The process must hold the `service` lifetime lock, or be `dogsled serve` itself, which holds it for its whole run (`src/service.rs:139`). This makes sure no older service on the host is still using the JSON files.
2. It takes the collection's kernel lock. Older binaries take the same lock for every write (all `save_queue` callers hold `queue`; all item writers hold `items`).
3. If the collection is `imported` but not `sealed`, and the JSON file is not a tombstone, it deletes the imported rows and imports again. Before the tombstone, the JSON file is still the authority.
4. It reads the JSON file with today's rules (an array; for the queue, also the version-1 envelope and the version check on `queue-format.json`), inserts every record in file order, and records `imported` with the file's SHA-256, in one transaction.
5. It copies the JSON file to `<name>.pre-sqlite.json` and fsyncs it, as a backup.
6. It replaces the JSON file with a tombstone, then marks the collection `sealed`.

Tombstones make every older binary fail closed instead of starting from an empty collection. A missing file would be read as empty (`src/items.rs:215` `unwrap_or_default`), so the file must never be missing.

| File | Tombstone | Older readers that refuse |
|---|---|---|
| `items.json` | `{"moved_to":"state.db","schema":1}` | Every older binary: an object does not decode as `Vec<Item>`. |
| `queue.json` | `{"version":2,"moved_to":"state.db","tickets":[]}` | Array readers: decode error. Envelope readers: version 2 is newer than 1. |
| `queue-format.json` | `{"version":2}` | Sidecar readers refuse before they read the queue (`src/work.rs:150`, test `a_newer_queue_sidecar_refuses_writes_without_changing_the_files`). |

If a new binary finds a collection not `sealed` and cannot take the `service` lock (an older service is running), it refuses that collection's operations with an error that says to restart the dogsled service. It never reads or writes the JSON file. This keeps one code path for live state and no compatibility shim.

### Rollout

- Stop the old service, install the new release, then start the new service. The first start migrates. README already asks for the same release on every host (`README.md:11`).
- Canary on one host first. Before and after, record the timings from step 0 and the row counts against the JSON array lengths.
- Hosts migrate independently. No host reads another host's files, so a mixed fleet works during rollout.

### Rollback

A downgrade after migration fails closed, by design. Decision D1 below chooses the way back.

## Steps

Each step is one writer unit. Step 2 starts after step 1 merges.

0. **Measure the current cost.** Scout, no source changes. On a copy of the local state directory (`DOGSLED_STATE=<copy>`), time one item write and one ticket write with the current release. Record count, median, and p90 in this plan. No other evidence for a speed gain exists yet.
1. **Store and items.** Add `rusqlite` with `bundled`; add `src/store.rs`; move `src/items.rs` onto it; migrate `items.json`. Move the direct `items.json` test writes onto a test helper in the store owner: `src/inbox.rs:1520–1524`, `src/sweep.rs:826`, `src/custody.rs:833–838`, the `loss_tests.rs` sites, and the `items.json` entry in `tests/cli/legacy_continue.rs:249–256`.
2. **Queue.** Move the queue functions in `src/work.rs` onto the store; remove the `queue-format.json` write; migrate `queue.json` and its sidecar in one transaction. Move the direct `queue.json` writes in `src/work_tests.rs`, `src/done/tests.rs:1068`, and `tests/cli/legacy_continue.rs`.
3. **Docs.** README state-directory section: `state.db`, the tombstones, the restart rule, and the rollback from D1.

## Tests

Each unit adds tests that fail before its change.

| Area | Case | Expected |
|---|---|---|
| Round trip | Current records, records from before optional fields, and records with fields this binary does not know | Same serde output as the JSON store. Old-record tests in `src/items.rs` tests still pass unchanged. |
| Row writes | One item change in a large collection | Only that row's `record` changes. Other rows keep their bytes. |
| Migration | Array queue, envelope queue, no sidecar, newer sidecar | Imports match the files in order. A newer sidecar refuses and changes nothing. |
| Crash | Stop after the import commit, after the backup, after the tombstone, before the seal (debug barrier like `launch_test_barrier`, `src/work.rs:1621`) | A rerun converges. No record is lost or duplicated. The JSON file is never missing. |
| Old binaries | Decode each tombstone with the pre-change readers, kept as test fixtures | Each refuses. None returns an empty collection. |
| Service lock | Collection unsealed while another process holds `service` | The new CLI refuses with the restart message and writes nothing. |
| Concurrency | Several `dogsled` processes enqueue and record at once | Every write lands. No `SQLITE_BUSY` reaches the caller inside the budget. |
| Versions | `user_version` newer than this binary | Every store operation refuses. The file is unchanged. |

Acceptance for each unit: `cargo test`, `cargo clippy --all-targets -- -D warnings`, and `cargo fmt --check` (the `moon.yml` precommit tasks).

## Order with R1

R1 deletes finished items and tickets older than 30 days, each by its own store (`plans/lifecycle-cleanup.md`, section 7). It shrinks the files but does not remove the two-file queue write or the full rewrite per change. Decision D2 below sets the order.

## Decisions

- **D1 — Way back after migration.** Recommended: add `dogsled state export`, which writes the current rows back as the legacy JSON arrays and removes the tombstones, so an older release can start again. Alternative: no export; restore `*.pre-sqlite.json` by hand and accept that changes made after migration are lost.
- **D2 — Order with R1.** Recommended: this plan first, so R1 is a `DELETE` in one transaction per store. Alternative: R1 first on the JSON files, and this plan after it.

## Research note: dependency choice

Checked 2026-10-29 against primary sources.

| Fact | Value | Source |
|---|---|---|
| `rusqlite` latest | 0.41.1, 2026-10-28, MIT, `rust-version` 1.89 (Dogsled needs 1.99) | [crates.io API](https://crates.io/api/v1/crates/rusqlite/0.41.1) |
| `bundled` feature | Enables `libsqlite3-sys/bundled` and `modern_sqlite` | same |
| `backup` feature | Present, for a later `dogsled state backup` | same |
| `libsqlite3-sys` | 0.39.1, MIT, bundles SQLite 3.54.1 | [crates.io API](https://crates.io/api/v1/crates/libsqlite3-sys/0.39.1), [bundled `sqlite3.h`](https://docs.rs/crate/libsqlite3-sys/0.39.1/source/sqlite3/sqlite3.h) |
| WAL-reset corruption bug | Present through 3.51.2, fixed in 3.51.3. Bundling 3.54.1 avoids an older system library. | [WAL](https://sqlite.org/wal.html#the_wal_reset_bug) |
| C toolchain | `bundled` compiles SQLite with `cc`. `cc` is already in `Cargo.lock`. The `plugin/fetch.sh` source fallback (`cargo build --release --locked`) now also needs a C compiler on the host. | `Cargo.lock`, `plugin/fetch.sh:55` |

Not chosen:

- **Turso embedded.** By default it refuses a second process on the same file. Multi-process mode is experimental and excludes its concurrent-write mode ([multi-process access](https://docs.turso.tech/sql-reference/multiprocess-access)). Dogsled opens the state from many processes: `serve`, `web`, `mcp`, `hook`, and every CLI call (`src/main.rs:190`, `:233`, `:241`, `:271`).
- **redb.** The released 4.4.0 README does not claim multi-process access ([v4.4.0](https://github.com/cberner/redb/blob/v4.4.0/README.md)).
- **sqlx.** Async only. Dogsled's state code is synchronous, so it would bring a runtime into every store call. Not checked further.
- **PostgreSQL or another central database.** It would change the host-ownership model in `src/node.rs`. That is a separate architecture decision.
