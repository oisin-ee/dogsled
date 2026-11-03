# Brief: Dioxus web client, embedded bundle, board UI, build pipeline (web portal step 2b)

**Outcome.** Opening `http://127.0.0.1:7465/` shows a Rust/WASM Dioxus page with the Dogsled board grouped by repository (collapsible repos, rows grouped To land / Attention / Running / Waiting with state pills and a small host label, todos per repo), live-updated from `/api/stream`; the bundle is built by `dx` and embedded in the `dogsled` binary, in CI, release and `plugin/fetch.sh`.

**Design.** Plan `plans/web-portal.md`, Approach (UI layout) and Files to modify. Dioxus 0.7.10 web only, not fullstack (cargo-dist builds `dogsled`, so `dx` must not own the server binary). `dx bundle --web` writes `public/` (`index.html`, `assets/`, `wasm/`): https://raw.githubusercontent.com/DioxusLabs/docsite/main/docs-src/0.7/src/tutorial/bundle.md and `.../guides/platforms/web.md`. Pin the output directory in `web/Dioxus.toml` so `rust-embed` has a fixed folder. DTOs come from `portal-types` (step 2a). Colours: the Catppuccin values in `src/sidebar/palette.rs` as CSS custom properties (Herdr theme read can come later). No terminal emulator, no JS framework.

**Edits.**
- New `web/` crate: `dioxus = { version = "0.7.10", features = ["web", "router"] }`, `portal-types`, `serde_json`, `web-sys`/`gloo-net` only if needed for `EventSource`/`fetch`. Components: `App` (three columns: left repo board, centre placeholder "Select an agent", right placeholder), `RepoGroup`, `Row`, `TodoList`, a "Needs you" count (rows in Attention). Initial `GET /api/board`, then `EventSource("/api/stream")` event `board`.
- Root `Cargo.toml`: add `web` to workspace members; add `rust-embed = "8.13.0"`. `web` must not be a dependency of `dogsled`.
- `src/web/assets.rs` (+ route in `src/web/mod.rs`): serve the embedded bundle at `/` and `/assets/*`, `/wasm/*` with correct MIME types (including `application/wasm`); fall back to `index.html` for unknown non-`/api` paths. If the bundle folder is missing at compile time, the build fails with a message naming `dx bundle --web`.
- `rust-toolchain.toml`: add `targets = ["wasm32-unknown-unknown"]`.
- `.prototools`: add `"cargo:dioxus-cli" = "0.7.10"`.
- `moon.yml`: `web-lint` (`cargo clippy -p web --target wasm32-unknown-unknown -- -D warnings`) and `web-build` (`dx bundle --web --release` in `web/`), inputs `web/**/*`, `portal-types/**/*`; `lint` and `test` depend on `web-build`. Exclude `web` from host-target clippy/test if it does not build natively (`--exclude web`).
- `.github/workflows/ci.yml`: `proto install` provides `dx`; add the wasm target; run `web-build` before cargo tasks.
- `dist-workspace.toml`: `github-build-setup` step running `dx bundle --web --release` before the cargo-dist build (see cargo-dist `github-build-setup` docs).
- `plugin/fetch.sh`: on the source-build path require `dx` (same error style as the `cargo` check) and build `web/` first.
- `.gitignore`: the bundle output folder.

**Owned paths.** `web/`, `Cargo.toml`, `Cargo.lock`, `src/web/assets.rs`, `src/web/mod.rs` (route only), `rust-toolchain.toml`, `.prototools`, `moon.yml`, `.github/workflows/ci.yml`, `.github/build-setup.yml` (if dist needs it), `dist-workspace.toml`, `plugin/fetch.sh`, `.gitignore`.

**Acceptance.** `proto install && moon run :web-lint :web-build :lint :test` exits 0; `dogsled web` then `curl -s 127.0.0.1:7465/ | grep -c wasm` ≥ 1 and `curl -sI 127.0.0.1:7465/wasm/<file>.wasm` shows `content-type: application/wasm`. Attach a screenshot of the board in a browser (headless Chrome or Playwright is fine) showing at least two repo groups.

**Checks.** The acceptance moon tasks. Do not run release builds of cargo-dist locally; validate `dist-workspace.toml` with `dist plan` if `dist` is installed, otherwise say so.

**Base.** origin/main after step 2a lands (the lead fills the SHA at dispatch).
