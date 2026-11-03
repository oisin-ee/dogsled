# Brief: `dogsled web` server, shared DTOs, board API (web portal step 2a)

**Outcome.** `dogsled web` serves a guarded JSON + SSE API on `127.0.0.1:<port>`: `GET /api/board` returns rows grouped by repository with todos, and `GET /api/stream` pushes the same payload when it changes. No UI yet (step 2b).

**Design.** Plan `plans/web-portal.md`, Approach 2 and 6. Reuse the sidebar read path exactly: `src/sidebar/mod.rs:45` `read` = `commands::status(fleet)` → `board::build(&status, owner, now)` → `board::cached_todos(fleet.ledger)`; here `owner = None` (all rows). It never probes; it reads cached ledger state. Rebuild every 2 s like `sidebar::REFRESH` (`src/sidebar/mod.rs:29`) and push only when the serialised payload differs. `Row.repository` exists after step 1 (`src/board.rs`). Dispatch pattern: `src/main.rs:195` (`Command::Board`).

**Edits.**
- `Cargo.toml`: make the repo a cargo workspace (`[workspace] members = [".", "portal-types"]`, keep the root package and its `[package.metadata.dist]`, lints and `[profile.dist]`). Add `axum = "0.8.9"`, `portal-types = { path = "portal-types" }`, and tokio features `rt-multi-thread`, `net`, `sync`, `time`, `macros`, `signal`. Add `tower` only if axum's test helpers need it (`tower::ServiceExt::oneshot`, dev-dependency).
- New `portal-types/` (lib, `serde` only, no tokio/axum so it compiles to wasm32 later): `BoardView { hosts: Vec<HostView>, repos: Vec<RepoGroup>, generated_ms: u64 }`, `RepoGroup { repository: Option<String> /* None = "Unsorted" */, rows: Vec<RowView>, todos: Vec<TodoView> }`, plus `RowView`, `HostView`, `TodoView` mirroring the fields of `board::Row`, `board::HostLine`, `board::TodoSections`. Conversions live in dogsled (`From` impls in `src/web/board.rs`), not in `portal-types`.
- Grouping: repos in first-appearance board order; rows keep `board::build` order inside each repo; rows with `repository: None` go to one trailing `None` group. A todo section belongs to the repo of the row whose `pane` equals `TodoSections.pane`; otherwise to the `None` group.
- `src/config.rs`: add `[web] port` (`u16`, default `7465`) in the same `#[serde(default, deny_unknown_fields)]` style; update config tests.
- `src/cli.rs` + `src/main.rs`: `Command::Web { port: Option<u16> }` → `web::serve(...)`. Build a tokio multi-thread runtime inside `web::serve`; the rest of dogsled stays sync. Run blocking reads (`commands::status` etc.) with `spawn_blocking`.
- New `src/web/mod.rs`: router, bind `127.0.0.1` only, graceful shutdown on SIGINT/SIGTERM. Guards as middleware: reject a `Host` header that is not `127.0.0.1:<port>` or `localhost:<port>` with 403; on POST/PUT/DELETE require `Origin` equal to `http://127.0.0.1:<port>` or `http://localhost:<port>` (403 otherwise). Create a per-user token file `<ledger dir>/web/token` (mode 0600, random 32 bytes hex, created once, reused) and expose `fn token(ledger)` for steps 4/5; no route uses it yet.
- New `src/web/board.rs`: grouping + conversions + `GET /api/board` + `GET /api/stream` (axum SSE, event name `board`, keep-alive 15 s).

**Owned paths.** `Cargo.toml`, `Cargo.lock`, `portal-types/`, `src/web/`, `src/cli.rs`, `src/main.rs`, `src/config.rs`, `src/lib.rs` (module registration, if the crate has one).

**Acceptance.** `cargo test --workspace` passes, including tests that: (1) group rows from two repos plus an unknown into three groups in board order with todos attached by pane; (2) `GET /api/board` with `Host: 127.0.0.1:<port>` returns 200 JSON and with `Host: evil.example` returns 403; (3) a POST without the portal `Origin` returns 403; (4) the token file is created 0600 and reused. Also report `dogsled web` started by hand, then `curl -s 127.0.0.1:7465/api/board | jq '.repos[].repository'` output.

**Checks.** `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` (or the matching moon tasks if step 0 has landed on the base).

**Base.** origin/main after steps 0 and 1 land (the lead fills the SHA at dispatch).
