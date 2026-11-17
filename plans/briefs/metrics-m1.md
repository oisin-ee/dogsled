# M1: metrics core, CLI, and API

Plan: `plans/metrics.md` (approved). Read it all before you edit. This brief names the files; the plan defines each table, rule, and metric.

## Outcome

`dogsled serve` keeps `metrics.db` current from this host's transcripts and `state.db`, and `dogsled metrics` and `GET /api/metrics` report tool calls, owner wait, and runs for the whole fleet.

## Design

- Wire types exist: `portal-types/src/metrics.rs` (`MetricsReport`, `Histogram::record/merge/quantile`). Do not change their shape; M2 builds the portal page on them in parallel.
- D2: `metrics.db` is its own store, not `state.db`. Model the connection, version check, and drop-and-recreate on `src/store.rs` `open`, but never call `service::wake::notify`.
- Transcript decoding: `src/activity.rs` already reads Pi, Claude, and Codex entries (call and result blocks near line 1018, `result_status`). Move the per-entry rules into one decoder that the conversation reader and the indexer both use. Do not write a second parser.
- Roots per harness: the ones `src/transcript.rs` `resolve` uses.
- Pane bindings: `src/observation/details.rs` `Session` (pane, harness, transcript).
- Peers: `fleet.remote(target, &["metrics", "--json", …])` as `src/web/agents.rs` `peer_call` does (line 315).
- D5: add `decided_ms` to `web_reviews` (`src/web_store.rs` line 111), set it where the decision is stored in `src/web_store/reviews.rs`, and bump `VERSION` in `src/store.rs` to 6.

## Edits

1. `src/metrics/mod.rs`, `store.rs`, `index.rs`, `harvest.rs`, `report.rs`: the store (including `input_entries`), indexer, harvest, 30-day delete, and the `MetricsReport` builder. The plan's "Input entries" section defines the ask and blocked wait.
2. `src/activity.rs`: the shared decoder move.
3. `src/service.rs`: `Lane::Metrics` (index, harvest, delete), recorded in `service_lanes` like the other lanes.
4. `src/cli.rs`, `src/main.rs`: `dogsled metrics [--window day|week|month] [--group …] [--tool NAME] [--json]`. Tables without `--json`.
5. `src/web/metrics.rs` and its route in `src/web/mod.rs` (near line 241): `GET /api/metrics?window=&group=&tool=`, local report plus peers in parallel, merged; a failed peer is a `HostCoverage` with its error.
6. `src/web_store.rs`, `src/web_store/reviews.rs`, `src/store.rs`: D5.
7. `README.md`: add `dogsled metrics` where the commands are listed.

## Paths

Owns: `src/metrics/`, `src/activity.rs`, `src/service.rs`, `src/cli.rs`, `src/main.rs`, `src/web/metrics.rs`, `src/web/mod.rs`, `src/web_store.rs`, `src/web_store/`, `src/store.rs`, `README.md`, test fixtures under `tests/fixtures/metrics/`.
Reads: everything else. Do not edit `web/`, `portal-types/`, or `pi/`.

## Acceptance

1. The tests in the plan's "Tests" table for M1 pass, including the input-entry case.
2. `cargo clippy --all-targets -- -D warnings` and `cargo fmt --check` are clean.

Do not build or run a release binary. The lead builds the branch once at landing and checks `dogsled metrics` and `/api/metrics` on the live hosts.

Report the exact commands and their output in your reply.

## Checks

`cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, and `cargo test metrics` plus the tests of `src/activity.rs` and `src/web_store`. Do not run the full suite.

## Base

The base of your branch: `origin/main` at dispatch.
