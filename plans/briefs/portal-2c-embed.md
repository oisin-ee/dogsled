# Brief: serve the embedded Dioxus bundle from `dogsled web`

**Outcome.** `dogsled web` serves the Dioxus page at `http://127.0.0.1:7465/` from the binary itself. Today `src/web/assets.rs` is a stub that returns 404.

**Design.**
- `moon run :web-build` runs `dx bundle --web --release` in `web/`. It writes `web/public/`, which holds `index.html`, `assets/` and `wasm/`. That folder is gitignored.
- CI already runs the web build before the lint and test steps. `plugin/fetch.sh` and the release build-setup also build web first.
- The fallback is already mounted in `src/web/mod.rs` as `.fallback(assets::serve)`, under the guard.

**Edits.**
- `Cargo.toml`: add `rust-embed = "8.13.0"` to dogsled. Do not enable debug-embed, so that every build embeds the files. Update `Cargo.lock`.
- `src/web/assets.rs`:
  - Add `#[derive(RustEmbed)] #[folder = "web/public/"]`.
  - `serve(uri)` returns the named file with its MIME type. Cover at least html, js, wasm (`application/wasm`), css, svg, png and ico.
  - Unknown paths outside `/api` fall back to `index.html`. Unknown `/api/*` paths stay 404.
  - Hashed asset names get `Cache-Control: public, max-age=31536000, immutable`. `index.html` gets `no-cache`.
  - If the folder is missing at compile time, the build fails with a message that names `moon run :web-build`.
- `moon.yml`: `lint` and `test` depend on `web-build`, so a fresh checkout builds. `fmt-check` stays independent.
- `README.md`, dev section only: say that `moon run :web-build` must run before a plain `cargo build`.

**Owned paths:** Cargo.toml, Cargo.lock, src/web/assets.rs, build.rs, moon.yml, README.md.

**Acceptance.**
1. Delete `web/public`, then run `moon run :lint :test`. It exits 0.
2. Tests show:
   - `GET /` returns HTML that contains `wasm`.
   - A `.wasm` file returns `application/wasm`.
   - `GET /some/route` returns `index.html`.
   - `GET /api/nope` returns 404.
3. Start `dogsled web`. Report the output of `curl -s 127.0.0.1:7465/ | grep -c wasm`, and of `curl -sI` on the wasm path from `index.html`.
