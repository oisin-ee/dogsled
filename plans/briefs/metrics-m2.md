# M2: portal Metrics page

Plan: `plans/metrics.md` (approved), section "Portal page". Read it before you edit.

## Outcome

The portal has a Metrics view that shows the fleet's tool calls, owner wait, and agent runs from `GET /api/metrics`.

## Design

- The data shape is fixed: `portal-types/src/metrics.rs` (`MetricsReport`, `ToolRow`, `WaitRow`, `RunRow`, `Histogram::quantile`). Do not change it. M1 builds the server route in parallel.
- Route: `GET /api/metrics?window=day|week|month&group=tool|repository|model|host|lead|harness&tool=NAME`. Fetch it like the other calls in `web/src/api.rs` (for example `models`, line 227).
- Entry point: a header button beside "New" (`web/src/app.rs` line 498) opens the view in `main`, the same way `composing` opens `NewLead` (lines 397–415 and 574). Opening an agent closes it.
- Look: follow `.impeccable/surfaces/web.md` and `DESIGN.md`. Use the components in `web/src/components/` (`tabs`, `select`, `card`, `badge`, `skeleton`).

## The view

1. Header: window selector (day, week, month) and one badge per `HostCoverage`. A host with `error` shows the error text, not a silent gap (`PRODUCT.md` principle 4).
2. Tools table: tool, group value (when the group is not `tool`), calls, error % (`errors / (calls − unknown_status − running)`; "—" when the divisor is 0), p50, p95, max. Click a column header to sort. A group selector and a tool filter re-fetch.
3. Owner wait: one row per `WaitKind` with answered, open, p50, p95.
4. Runs: one row per group with outcome counts and p50 and p95 of queue wait, spawn, and time to done.
5. Durations in human units (`850 ms`, `12 s`, `5.7 min`, `1.2 h`).
6. Loading shows a `Skeleton`; a failed fetch shows the error and a retry button.

## Edits

- `web/src/metrics.rs`: the view.
- `web/src/api.rs`: `fetch_metrics`.
- `web/src/app.rs`: the header button and the `main` branch.
- `web/src/main.rs`: the module.
- `web/assets/` styles for the view, if the page needs any.
- `web/src/component_tests.rs`: one test that renders a recorded `MetricsReport` and checks the error % and the p95 text. Nothing else needs a test.

## Paths

Owns: `web/src/metrics.rs`, `web/src/api.rs`, `web/src/app.rs`, `web/src/main.rs`, `web/src/component_tests.rs`, `web/assets/`.
Reads: everything else. Do not edit `src/`, `portal-types/`, or `pi/`.

## Acceptance

1. The component test passes.
2. With a `dogsled web` built from M1's branch or from `main` after M1 lands, the view loads, and window, group, and sort change the table. If M1 is not on `main` when you finish, report that this step waits on M1.

## Checks

`cargo fmt --check`, `cargo clippy -p web --target wasm32-unknown-unknown -- -D warnings`, and `cargo test -p web component_tests`. Do not run the full suite or `dx bundle`.

## Base

The base of your branch: `origin/main` at dispatch.
